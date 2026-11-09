use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sha2::{Digest, Sha256};

use crate::StorageError;

/// Content-addressed WebP covers beside the database.
#[derive(Debug)]
pub struct CoverCache {
    dir: PathBuf,
}

impl CoverCache {
    pub(crate) fn open(database_path: &Path) -> Result<Self, StorageError> {
        let dir = sibling_dir(database_path, "covers")?;
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// Store WebP bytes. The file name is the SHA-256 of those bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when the bytes are not WebP or the write fails.
    pub fn insert(&self, webp: &[u8]) -> Result<PathBuf, StorageError> {
        if !is_webp(webp) {
            return Err(StorageError::NotWebp);
        }
        write_addressed(&self.dir, webp, "webp")
    }
}

/// Content-addressed page files with a least-recently-used cap.
#[derive(Debug)]
pub struct PageCache {
    inner: Mutex<PageInner>,
}

#[derive(Debug)]
struct PageInner {
    dir: PathBuf,
    max_entries: usize,
    order: Vec<String>,
}

impl PageCache {
    pub(crate) fn open(database_path: &Path, max_entries: usize) -> Result<Self, StorageError> {
        if max_entries == 0 {
            return Err(StorageError::CacheLimit);
        }
        let dir = sibling_dir(database_path, "pages")?;
        fs::create_dir_all(&dir)?;
        let order = read_order(&dir.join("lru.txt"))?;
        Ok(Self {
            inner: Mutex::new(PageInner {
                dir,
                max_entries,
                order,
            }),
        })
    }

    /// Store page bytes and evict the least recently used file when over the cap.
    ///
    /// # Errors
    ///
    /// Returns an error when the cache lock is poisoned or the write fails.
    pub fn insert(&self, bytes: &[u8]) -> Result<PathBuf, StorageError> {
        let mut inner = self.inner.lock().map_err(|_| StorageError::LockPoisoned)?;
        let digest = digest_hex(bytes);
        let path = inner.dir.join(format!("{digest}.page"));
        if !path.is_file() {
            write_atomic(&inner.dir, &digest, "page", bytes)?;
        }
        if let Some(index) = inner.order.iter().position(|entry| entry == &digest) {
            inner.order.remove(index);
        }
        inner.order.push(digest);
        while inner.order.len() > inner.max_entries {
            let evicted = inner.order.remove(0);
            let evicted_path = inner.dir.join(format!("{evicted}.page"));
            if evicted_path.is_file() {
                fs::remove_file(evicted_path)?;
            }
        }
        write_order(&inner.dir.join("lru.txt"), &inner.order)?;
        Ok(path)
    }
}

fn sibling_dir(database_path: &Path, name: &str) -> Result<PathBuf, StorageError> {
    let Some(parent) = database_path.parent() else {
        return Err(StorageError::Path);
    };
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    Ok(parent.join(name))
}

fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

fn digest_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let high = usize::from(byte >> 4);
        let low = usize::from(byte & 0x0f);
        out.push(char::from(DIGITS[high]));
        out.push(char::from(DIGITS[low]));
    }
    out
}

fn write_addressed(dir: &Path, bytes: &[u8], extension: &str) -> Result<PathBuf, StorageError> {
    let digest = digest_hex(bytes);
    write_atomic(dir, &digest, extension, bytes)?;
    Ok(dir.join(format!("{digest}.{extension}")))
}

fn write_atomic(
    dir: &Path,
    digest: &str,
    extension: &str,
    bytes: &[u8],
) -> Result<(), StorageError> {
    if !is_digest(digest) {
        return Err(StorageError::Path);
    }
    let partial = dir.join(format!("{digest}.{extension}.partial"));
    let final_path = dir.join(format!("{digest}.{extension}"));
    fs::write(&partial, bytes)?;
    if let Err(error) = fs::rename(&partial, &final_path) {
        if let Err(remove_error) = fs::remove_file(&partial) {
            eprintln!("cache partial remove failed: {remove_error}");
        }
        return Err(error.into());
    }
    Ok(())
}

fn is_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn read_order(path: &Path) -> Result<Vec<String>, StorageError> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(path)?;
    Ok(text
        .lines()
        .filter(|line| is_digest(line))
        .map(str::to_owned)
        .collect())
}

fn write_order(path: &Path, order: &[String]) -> Result<(), StorageError> {
    let mut text = String::new();
    for digest in order {
        text.push_str(digest);
        text.push('\n');
    }
    let partial = path.with_extension("txt.partial");
    fs::write(&partial, text)?;
    if let Err(error) = fs::rename(&partial, path) {
        if let Err(remove_error) = fs::remove_file(&partial) {
            eprintln!("cache order remove failed: {remove_error}");
        }
        return Err(error.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{is_webp, CoverCache, PageCache};

    fn webp_header() -> Vec<u8> {
        let mut bytes = b"RIFF----WEBP".to_vec();
        bytes.extend_from_slice(b"page");
        bytes
    }

    #[test]
    fn cover_cache_rejects_png_and_names_webp_by_digest() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("folio.sqlite");
        let cache = CoverCache::open(&database).unwrap();
        assert!(matches!(
            cache.insert(b"\x89PNG not webp"),
            Err(crate::StorageError::NotWebp)
        ));
        let first = cache.insert(&webp_header()).unwrap();
        let second = cache.insert(&webp_header()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.extension().and_then(|ext| ext.to_str()), Some("webp"));
        assert!(is_webp(&webp_header()));
    }

    #[test]
    fn page_cache_evicts_least_recently_used() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("folio.sqlite");
        let cache = PageCache::open(&database, 2).unwrap();
        let first = cache.insert(b"one").unwrap();
        let _second = cache.insert(b"two").unwrap();
        let _third = cache.insert(b"three").unwrap();
        assert!(!first.is_file());
        assert!(cache.insert(b"two").unwrap().is_file());
    }
}
