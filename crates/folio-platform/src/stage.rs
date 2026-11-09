//! Bounded copy of a stream that cannot seek.
//!
//! Android providers sometimes return a pipe. Parsers need a
//! [`SeekableSource`](folio_core::SeekableSource), so the bytes are copied
//! into one file under a caller-owned directory. The copy is removed when
//! the staged source is dropped, when the copy is cancelled, and when it
//! would grow past the limit.

use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use folio_core::SeekableSource;
use thiserror::Error;

/// A seekable file that deletes itself on drop.
#[derive(Debug)]
pub struct StagedSource {
    file: Option<File>,
    path: PathBuf,
}

impl SeekableSource for StagedSource {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let file = self.file()?;
        file.seek(SeekFrom::Start(offset))?;
        file.read(buf)
    }

    fn len(&self) -> io::Result<u64> {
        let file = self.file.as_ref().ok_or_else(closed)?;
        Ok(file.metadata()?.len())
    }
}

impl Drop for StagedSource {
    fn drop(&mut self) {
        self.file.take();
        if self.path.as_os_str().is_empty() {
            return;
        }
        if let Err(error) = fs::remove_file(&self.path) {
            if error.kind() != ErrorKind::NotFound {
                tracing::warn!(%error, "staged copy could not be removed");
            }
        }
    }
}

impl StagedSource {
    fn file(&mut self) -> io::Result<&mut File> {
        self.file.as_mut().ok_or_else(closed)
    }
}

/// Why a non-seekable stream was not staged.
#[derive(Debug, Error)]
pub enum PlatformError {
    /// `cancel` was set before the copy finished.
    #[error("the staged copy was cancelled")]
    Cancelled,
    /// The stream is longer than `limit`.
    #[error("the staged copy exceeded the byte limit")]
    LimitExceeded,
    /// The directory or the copy could not be used.
    #[error("the staged copy could not be stored")]
    Io(#[source] io::Error),
    /// The grant id or URI is empty or contains a newline or tab.
    #[error("the grant record is invalid")]
    InvalidGrant,
    /// No grant is stored for that tree.
    #[error("the tree grant is not stored")]
    NotGranted,
}

impl From<io::Error> for PlatformError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Copy `reader` into `directory/staged.bin` and return a seekable view.
///
/// `directory` holds this one copy. A second call with the same directory
/// replaces nothing safely: use a fresh directory per stream.
///
/// The file is removed if `cancel` becomes true, if the next byte would pass
/// `limit`, or if a later read or write fails. [`StagedSource`] removes it
/// again on drop. A limit of zero accepts an empty stream and rejects any byte.
///
/// # Errors
///
/// Returns [`PlatformError::Cancelled`], [`PlatformError::LimitExceeded`], or
/// [`PlatformError::Io`]. When the error is cancellation or the limit, the
/// directory has no `staged.bin` left from this call.
pub fn stage_unseekable(
    reader: &mut dyn Read,
    directory: &Path,
    limit: u64,
    cancel: &AtomicBool,
) -> Result<StagedSource, PlatformError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(PlatformError::Cancelled);
    }
    let path = directory.join("staged.bin");
    // Read and write: parsers seek back over the copy. Create is write-only.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)?;
    let mut partial = PartialFile {
        file: Some(file),
        path,
    };
    let mut written = 0u64;
    let mut buf = [0u8; 8192];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return partial.fail(PlatformError::Cancelled);
        }
        let read = match partial.read_from(reader, &mut buf) {
            Ok(count) => count,
            Err(error) => return partial.fail(PlatformError::Io(error)),
        };
        if read == 0 {
            break;
        }
        if cancel.load(Ordering::Relaxed) {
            return partial.fail(PlatformError::Cancelled);
        }
        let add =
            u64::try_from(read).map_err(|error| io::Error::new(ErrorKind::InvalidInput, error))?;
        let next = match written.checked_add(add) {
            Some(sum) => sum,
            None => {
                return partial.fail(PlatformError::Io(io::Error::new(
                    ErrorKind::InvalidInput,
                    "staged length overflow",
                )))
            }
        };
        if next > limit {
            return partial.fail(PlatformError::LimitExceeded);
        }
        if let Err(error) = partial.write_all(&buf[..read]) {
            return partial.fail(PlatformError::Io(error));
        }
        written = next;
    }
    if let Err(error) = partial.rewind() {
        return partial.fail(PlatformError::Io(error));
    }
    partial.into_staged()
}

struct PartialFile {
    file: Option<File>,
    path: PathBuf,
}

impl PartialFile {
    fn read_from(&mut self, reader: &mut dyn Read, buf: &mut [u8]) -> io::Result<usize> {
        reader.read(buf)
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.file.as_mut().ok_or_else(closed)?.write_all(bytes)
    }

    fn rewind(&mut self) -> io::Result<()> {
        self.file
            .as_mut()
            .ok_or_else(closed)?
            .seek(SeekFrom::Start(0))?;
        Ok(())
    }

    fn fail(mut self, error: PlatformError) -> Result<StagedSource, PlatformError> {
        self.file.take();
        let removed = remove_file(&self.path);
        self.path.clear();
        removed?;
        Err(error)
    }

    fn into_staged(mut self) -> Result<StagedSource, PlatformError> {
        let Some(file) = self.file.take() else {
            self.path.clear();
            return Err(PlatformError::Io(closed()));
        };
        let path = std::mem::take(&mut self.path);
        Ok(StagedSource {
            file: Some(file),
            path,
        })
    }
}

impl Drop for PartialFile {
    fn drop(&mut self) {
        self.file.take();
        if self.path.as_os_str().is_empty() {
            return;
        }
        if let Err(error) = fs::remove_file(&self.path) {
            if error.kind() != ErrorKind::NotFound {
                tracing::warn!(%error, "partial staged copy could not be removed");
            }
        }
    }
}

fn remove_file(path: &Path) -> Result<(), PlatformError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(PlatformError::Io(error)),
    }
}

fn closed() -> io::Error {
    io::Error::other("staged copy is closed")
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::stage_unseekable;
    use crate::PlatformError;
    use folio_core::SeekableSource;

    #[test]
    fn pipe_round_trip_matches_bytes() -> Result<(), PlatformError> {
        let payload = b"pipe-bytes-for-folio";
        let (mut reader, mut writer) = std::io::pipe()?;
        writer.write_all(payload)?;
        drop(writer);
        let dir = scratch("round")?;
        let limit = u64::try_from(payload.len()).map_err(io_invalid)?;
        let mut staged = stage_unseekable(&mut reader, &dir.0, limit, &AtomicBool::new(false))?;
        let mut got = vec![0u8; payload.len()];
        let read = staged.read_at(0, &mut got)?;
        assert_eq!(read, payload.len());
        assert_eq!(got, payload);
        let path = dir.0.join("staged.bin");
        assert!(path.is_file());
        drop(staged);
        assert!(!path.exists());
        Ok(())
    }

    #[test]
    fn over_limit_leaves_the_directory_empty() -> Result<(), PlatformError> {
        let payload = b"0123456789";
        let (mut reader, mut writer) = std::io::pipe()?;
        writer.write_all(payload)?;
        drop(writer);
        let dir = scratch("limit")?;
        let error = stage_unseekable(&mut reader, &dir.0, 9, &AtomicBool::new(false))
            .expect_err("over limit");
        assert!(matches!(error, PlatformError::LimitExceeded));
        assert!(dir.is_empty()?);
        Ok(())
    }

    #[test]
    fn cancel_removes_the_partial() -> Result<(), PlatformError> {
        let payload = b"0123456789abcdef";
        let cancel = AtomicBool::new(false);
        let mut reader = CancelOnSecond {
            data: payload,
            pos: 0,
            cancel: &cancel,
        };
        let dir = scratch("cancel")?;
        let error = stage_unseekable(&mut reader, &dir.0, 100, &cancel).expect_err("cancelled");
        assert!(matches!(error, PlatformError::Cancelled));
        assert!(dir.is_empty()?);
        Ok(())
    }

    struct CancelOnSecond<'a> {
        data: &'a [u8],
        pos: usize,
        cancel: &'a AtomicBool,
    }

    impl std::io::Read for CancelOnSecond<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.pos > 0 {
                self.cancel.store(true, Ordering::Relaxed);
            }
            if self.pos >= self.data.len() {
                return Ok(0);
            }
            let count = buf.len().min(self.data.len() - self.pos).min(4);
            buf[..count].copy_from_slice(&self.data[self.pos..self.pos + count]);
            self.pos += count;
            Ok(count)
        }
    }

    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn is_empty(&self) -> Result<bool, PlatformError> {
            Ok(std::fs::read_dir(&self.0)?.next().is_none())
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(&self.0) {
                eprintln!("scratch cleanup failed: {error}");
            }
        }
    }

    fn scratch(label: &str) -> Result<Scratch, PlatformError> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(io_invalid)?
            .as_nanos();
        let mut path = std::env::temp_dir();
        path.push(format!(
            "folio-stage-{label}-{nanos}-{}",
            std::process::id()
        ));
        std::fs::create_dir(&path)?;
        Ok(Scratch(path))
    }

    fn io_invalid(error: impl std::fmt::Display) -> PlatformError {
        PlatformError::Io(std::io::Error::other(error.to_string()))
    }
}
