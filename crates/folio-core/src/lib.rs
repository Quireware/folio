//! Shared types for content location, reading sources, and file kinds.
#![deny(missing_docs)]

use std::io::{self, ErrorKind};
use std::path::PathBuf;
use std::sync::Once;

use tracing_subscriber::EnvFilter;

/// Where a book file is found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentSource {
    /// A filesystem path. Desktop libraries use this.
    Path(PathBuf),
    /// A persistable Android document-tree URI.
    AndroidTree {
        /// The tree URI returned by `ACTION_OPEN_DOCUMENT_TREE`.
        uri: String,
    },
}

impl ContentSource {
    /// A filesystem path.
    pub fn path(path: impl Into<PathBuf>) -> Self {
        Self::Path(path.into())
    }

    /// An Android document-tree URI.
    pub fn android_tree(uri: impl Into<String>) -> Self {
        Self::AndroidTree { uri: uri.into() }
    }
}

/// Random-access bytes. Parsers use this instead of assuming a real path.
pub trait SeekableSource {
    /// Read bytes starting at `offset` into `buf`.
    ///
    /// A short read at the end of the source returns the bytes that were
    /// available, including zero when `offset` is exactly the length.
    ///
    /// # Errors
    ///
    /// Returns an error when `offset` is past the end or does not fit in `usize`.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;

    /// Length in bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when the length cannot be represented as `u64`.
    fn len(&self) -> io::Result<u64>;

    /// Whether [`SeekableSource::len`] is zero.
    ///
    /// # Errors
    ///
    /// Returns an error when the length cannot be read.
    fn is_empty(&self) -> io::Result<bool> {
        Ok(self.len()? == 0)
    }
}

/// A seekable view of an in-memory byte slice.
#[derive(Debug, Clone, Copy)]
pub struct SliceSource<'a> {
    bytes: &'a [u8],
}

impl<'a> SliceSource<'a> {
    /// Borrow `bytes` as a [`SeekableSource`].
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
}

impl SeekableSource for SliceSource<'_> {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let start = usize::try_from(offset)
            .map_err(|error| io::Error::new(ErrorKind::InvalidInput, error))?;
        if start > self.bytes.len() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "read offset is past the end",
            ));
        }
        let available = &self.bytes[start..];
        let count = buf.len().min(available.len());
        buf[..count].copy_from_slice(&available[..count]);
        Ok(count)
    }

    fn len(&self) -> io::Result<u64> {
        u64::try_from(self.bytes.len())
            .map_err(|error| io::Error::new(ErrorKind::InvalidInput, error))
    }
}

/// Book container kinds folio knows how to open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileType {
    /// EPUB publication.
    Epub,
    /// ZIP comic archive.
    Cbz,
    /// 7z comic archive.
    Cb7,
    /// TAR comic archive.
    Cbt,
    /// RAR comic archive.
    Cbr,
    /// PDF document.
    Pdf,
    /// MOBI, AZW, or PRC text.
    Mobi,
    /// Directory of page images.
    ImageFolder,
    /// Not a supported book.
    Unknown,
}

impl FileType {
    /// Extension first. Magic is used only when the extension is missing or unknown.
    ///
    /// ZIP magic is not enough to choose EPUB or CBZ, so an unknown extension
    /// with a ZIP prefix stays [`FileType::Unknown`].
    pub fn detect(name: &str, prefix: &[u8]) -> Self {
        extension_kind(name).unwrap_or_else(|| magic_kind(prefix).unwrap_or(Self::Unknown))
    }
}

fn extension_kind(name: &str) -> Option<FileType> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let (stem, extension) = base.rsplit_once('.')?;
    if stem.is_empty() || extension.is_empty() {
        return None;
    }
    if extension.eq_ignore_ascii_case("epub") {
        Some(FileType::Epub)
    } else if extension.eq_ignore_ascii_case("cbz") {
        Some(FileType::Cbz)
    } else if extension.eq_ignore_ascii_case("cb7") {
        Some(FileType::Cb7)
    } else if extension.eq_ignore_ascii_case("cbt") {
        Some(FileType::Cbt)
    } else if extension.eq_ignore_ascii_case("cbr") {
        Some(FileType::Cbr)
    } else if extension.eq_ignore_ascii_case("pdf") {
        Some(FileType::Pdf)
    } else if extension.eq_ignore_ascii_case("mobi")
        || extension.eq_ignore_ascii_case("azw")
        || extension.eq_ignore_ascii_case("azw3")
        || extension.eq_ignore_ascii_case("prc")
    {
        Some(FileType::Mobi)
    } else {
        None
    }
}

fn magic_kind(prefix: &[u8]) -> Option<FileType> {
    if prefix.starts_with(b"%PDF") {
        Some(FileType::Pdf)
    } else if prefix.starts_with(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]) {
        Some(FileType::Cb7)
    } else if prefix.starts_with(b"Rar!\x1A\x07") {
        Some(FileType::Cbr)
    } else if prefix.len() >= 262 && &prefix[257..262] == b"ustar" {
        Some(FileType::Cbt)
    } else if prefix.len() >= 68 && &prefix[60..64] == b"BOOK" && &prefix[64..68] == b"MOBI" {
        Some(FileType::Mobi)
    } else {
        None
    }
}

/// Install a process-wide tracing subscriber once.
///
/// A subscriber that is already installed is left in place. That is success:
/// tests and an embedding host may have installed their own.
pub fn init_tracing() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        let subscriber = tracing_subscriber::fmt().with_env_filter(filter).finish();
        if let Err(error) = tracing::subscriber::set_global_default(subscriber) {
            eprintln!("tracing subscriber already installed: {error}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::FileType;

    #[test]
    fn extension_wins_over_contradictory_magic() {
        assert_eq!(FileType::detect("Book.EPUB", b"%PDF-1.7"), FileType::Epub);
        assert_eq!(FileType::detect("scan.bin", b"%PDF-1.7"), FileType::Pdf);
        assert_eq!(FileType::detect("Book.mobi", b""), FileType::Mobi);
        assert_eq!(FileType::detect("Book.azw3", b""), FileType::Mobi);
        assert_eq!(FileType::detect("Book.prc", b""), FileType::Mobi);
        assert_eq!(
            FileType::detect("notes.bin", b"PK\x03\x04"),
            FileType::Unknown
        );
        assert_eq!(FileType::detect(".gitignore", b""), FileType::Unknown);
    }
}
