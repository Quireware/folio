//! Persistable document-tree grants. Revoking a grant does not delete progress.

use std::fs;
use std::path::{Path, PathBuf};

use crate::stage::PlatformError;

/// A tree the user granted, restored after process death.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeGrant {
    /// Stable id for this grant.
    pub id: String,
    /// Tree URI, or a desktop path standing in for one in tests.
    pub uri: String,
}

/// File-backed grant store. Opening it again is the process restart.
#[derive(Debug)]
pub struct GrantStore {
    path: PathBuf,
}

impl GrantStore {
    /// Store grants in `path`. The file is created on the first write.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Record a persistable tree grant. A second call for the same id replaces the URI.
    ///
    /// # Errors
    ///
    /// Returns an error when the id or URI is empty or contains a newline or tab,
    /// or when the file cannot be written.
    pub fn take_persistable(&self, id: &str, uri: &str) -> Result<(), PlatformError> {
        validate(id, uri)?;
        let mut grants = self.read()?;
        if let Some(existing) = grants.iter_mut().find(|grant| grant.id == id) {
            existing.uri = uri.to_owned();
        } else {
            grants.push(TreeGrant {
                // to_owned: the grant file stores the id
                id: id.to_owned(),
                uri: uri.to_owned(),
            });
        }
        self.write(&grants)
    }

    /// Grants stored by the last successful [`GrantStore::take_persistable`].
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or a line is corrupt.
    pub fn restore(&self) -> Result<Vec<TreeGrant>, PlatformError> {
        self.read()
    }

    /// URI for `id`.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::NotGranted`] when `id` is not stored.
    pub fn resolve(&self, id: &str) -> Result<String, PlatformError> {
        self.read()?
            .into_iter()
            .find(|grant| grant.id == id)
            .map(|grant| grant.uri)
            .ok_or(PlatformError::NotGranted)
    }

    /// Forget `id`. Other app data, including progress rows, is not touched.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be rewritten.
    pub fn revoke(&self, id: &str) -> Result<(), PlatformError> {
        let grants = self
            .read()?
            .into_iter()
            .filter(|grant| grant.id != id)
            .collect::<Vec<_>>();
        self.write(&grants)
    }

    fn read(&self) -> Result<Vec<TreeGrant>, PlatformError> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(PlatformError::Io(error)),
        };
        let mut grants = Vec::new();
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            let Some((id, uri)) = line.split_once('\t') else {
                return Err(PlatformError::InvalidGrant);
            };
            validate(id, uri)?;
            grants.push(TreeGrant {
                id: id.to_owned(),
                uri: uri.to_owned(),
            });
        }
        Ok(grants)
    }

    fn write(&self, grants: &[TreeGrant]) -> Result<(), PlatformError> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let mut body = String::new();
        for grant in grants {
            body.push_str(&grant.id);
            body.push('\t');
            body.push_str(&grant.uri);
            body.push('\n');
        }
        let temporary = temporary_path(&self.path);
        fs::write(&temporary, body)?;
        fs::rename(&temporary, &self.path)?;
        Ok(())
    }
}

fn validate(id: &str, uri: &str) -> Result<(), PlatformError> {
    if id.is_empty()
        || uri.is_empty()
        || id.contains(['\n', '\t', '\r'])
        || uri.contains(['\n', '\t', '\r'])
    {
        return Err(PlatformError::InvalidGrant);
    }
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use folio_storage::Database;

    use super::GrantStore;

    #[test]
    fn grant_survives_restart() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("folio-grant-{nanos}"));
        fs::create_dir(&root).expect("root");
        let books = root.join("books");
        fs::create_dir(&books).expect("books");
        fs::write(books.join("issue.pdf"), b"%PDF-1.4").expect("pdf");
        let uri = books.to_str().expect("utf-8 path");
        let grants = root.join("grants.txt");
        {
            let store = GrantStore::open(&grants);
            store.take_persistable("tree-1", uri).expect("grant");
        }
        let store = GrantStore::open(&grants);
        let restored = store.restore().expect("restore");
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].id, "tree-1");
        let resolved = store.resolve("tree-1").expect("resolve");
        let bytes = fs::read(std::path::Path::new(&resolved).join("issue.pdf")).expect("open item");
        assert!(bytes.starts_with(b"%PDF"));

        let db = Database::open(&root.join("folio.sqlite")).expect("db");
        db.set_progress("tree-1/issue.pdf", "page:1")
            .expect("progress");
        store.revoke("tree-1").expect("revoke");
        assert!(store.restore().expect("after revoke").is_empty());
        assert_eq!(
            db.progress("tree-1/issue.pdf").expect("read").as_deref(),
            Some("page:1")
        );
        drop(db);
        fs::remove_dir_all(&root).expect("cleanup");
    }
}
