//! Auth keys on disk. The database stores a reference, never these bytes.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Why a credential could not be stored or read.
#[derive(Debug, Error)]
pub enum SecretError {
    /// The kind or the server id was empty or contained a path separator.
    #[error("credential id must not be empty")]
    Empty,
    /// The file was missing.
    #[error("credential is missing")]
    Missing,
    /// The filesystem rejected the operation.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A directory of credential files. One file per server id.
#[derive(Debug, Clone)]
pub struct SecretStore {
    root: PathBuf,
}

impl SecretStore {
    /// Open `root/secrets`, creating it if needed.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory cannot be created.
    pub fn open(root: &Path) -> Result<Self, SecretError> {
        let root = root.join("secrets");
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    /// Replace the credential for `kind` and `server_id`.
    ///
    /// # Errors
    ///
    /// Returns an error when the id is empty or the file cannot be written.
    pub fn write(&self, kind: &str, server_id: &str, secret: &str) -> Result<(), SecretError> {
        let path = self.path(kind, server_id)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = File::options()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(secret.as_bytes())?;
        Ok(())
    }

    /// Read the credential.
    ///
    /// # Errors
    ///
    /// Returns [`SecretError::Missing`] when the file is absent, and [`SecretError::Empty`]
    /// when the id is not a single path segment.
    pub fn read(&self, kind: &str, server_id: &str) -> Result<String, SecretError> {
        let path = self.path(kind, server_id)?;
        if !path.exists() {
            return Err(SecretError::Missing);
        }
        let mut file = File::open(path)?;
        let mut body = String::new();
        file.read_to_string(&mut body)?;
        Ok(body)
    }

    /// Remove the credential. A missing file is success.
    ///
    /// # Errors
    ///
    /// Returns an error when the id is empty or the directory cannot be updated.
    pub fn delete(&self, kind: &str, server_id: &str) -> Result<(), SecretError> {
        let path = self.path(kind, server_id)?;
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn path(&self, kind: &str, server_id: &str) -> Result<PathBuf, SecretError> {
        if !single_segment(kind) || !single_segment(server_id) {
            return Err(SecretError::Empty);
        }
        Ok(self.root.join(kind).join(server_id))
    }
}

fn single_segment(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('/')
        && !value.contains('\\')
        && value != "."
        && value != ".."
}

#[cfg(test)]
mod tests {
    use super::SecretStore;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn key_roundtrip_is_mode_600_and_delete_removes_it() {
        let root = tempfile::tempdir().expect("temp");
        let store = SecretStore::open(root.path()).expect("store");
        store
            .write("kavita", "server-1", "raw-api-key")
            .expect("write");
        let path = root.path().join("secrets").join("kavita").join("server-1");
        let mode = fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(
            store.read("kavita", "server-1").expect("read"),
            "raw-api-key"
        );
        store.delete("kavita", "server-1").expect("delete");
        assert!(store.read("kavita", "server-1").is_err());
        assert!(!path.exists());
    }
}
