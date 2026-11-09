//! External SD volume identity.
//!
//! This is not a Phase 4 tree grant. A `content://` tree URI is rejected.

use thiserror::Error;

/// Why an external SD id was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AndroidError {
    /// The value is empty or is a tree-grant URI.
    #[error("external sd volume is not a tree grant")]
    NotExternalVolume,
}

/// An external SD volume, separate from [`crate::TreeGrant`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalSdRoot {
    volume_id: String,
}

impl ExternalSdRoot {
    /// Accept a volume id such as `1234-5678`.
    ///
    /// # Errors
    ///
    /// Returns [`AndroidError::NotExternalVolume`] when `volume_id` is empty or contains
    /// `://` or `/`, which is how a tree-grant URI is shaped.
    pub fn new(volume_id: &str) -> Result<Self, AndroidError> {
        if volume_id.is_empty() || volume_id.contains("://") || volume_id.contains('/') {
            return Err(AndroidError::NotExternalVolume);
        }
        Ok(Self {
            // to_owned: the volume id is stored
            volume_id: volume_id.to_owned(),
        })
    }

    /// Volume id.
    pub fn volume_id(&self) -> &str {
        &self.volume_id
    }
}

#[cfg(test)]
mod tests {
    use super::{AndroidError, ExternalSdRoot};
    use crate::TreeGrant;

    #[test]
    fn external_sd_root_rejects_a_tree_grant_uri() {
        let grant = TreeGrant {
            id: "tree".to_owned(),
            uri: "content://com.android.externalstorage.documents/tree/primary".to_owned(),
        };
        assert_eq!(
            ExternalSdRoot::new(&grant.uri),
            Err(AndroidError::NotExternalVolume)
        );
        assert_eq!(
            ExternalSdRoot::new(""),
            Err(AndroidError::NotExternalVolume)
        );
        let volume = ExternalSdRoot::new("1234-5678").expect("volume");
        assert_eq!(volume.volume_id(), "1234-5678");
        assert_ne!(volume.volume_id(), grant.uri.as_str());
    }
}
