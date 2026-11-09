//! UI projections loaded from repositories. They are not a second copy of the database.
#![deny(missing_docs)]

use folio_storage::{Database, StorageError};
use folio_sync::{ConflictPolicy, SyncState};
use thiserror::Error;

/// Failure while building a projection.
#[derive(Debug, Error)]
pub enum AppError {
    /// The database read failed.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// `conflict_policy` is not a known value.
    #[error("unknown conflict policy")]
    UnknownConflictPolicy,
}

/// Library rows the shell can list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryState {
    /// Item ids in storage order.
    pub item_ids: Vec<String>,
}

/// The newest saved reading position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReaderState {
    /// Locator of the most recently written progress row.
    pub locator: Option<String>,
}

/// Settings rows as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefsState {
    /// Key and value pairs.
    pub settings: Vec<(String, String)>,
}

/// Saved server rows. This does not open a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerState {
    /// Server ids in storage order.
    pub ids: Vec<String>,
}

/// Disposable view of the repositories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection {
    /// Library ids.
    pub library: LibraryState,
    /// Latest locator.
    pub reader: ReaderState,
    /// Sync preferences.
    pub sync: SyncState,
    /// Settings.
    pub prefs: PrefsState,
    /// Server ids.
    pub servers: ServerState,
}

/// Load projections from `db`. Signals are not written back.
///
/// # Errors
///
/// Returns an error when a read fails or `conflict_policy` is unknown.
pub fn load_projection(db: &Database) -> Result<Projection, AppError> {
    let settings = db.settings()?;
    Ok(Projection {
        library: LibraryState {
            item_ids: db.item_ids()?,
        },
        reader: ReaderState {
            locator: db.latest_locator()?,
        },
        sync: SyncState::new(conflict_policy(&settings)?),
        prefs: PrefsState { settings },
        servers: ServerState {
            ids: db.server_ids()?,
        },
    })
}

fn conflict_policy(settings: &[(String, String)]) -> Result<ConflictPolicy, AppError> {
    match settings
        .iter()
        .find(|(key, _value)| key == "conflict_policy")
    {
        None => Ok(ConflictPolicy::LastWriteWins),
        Some((_key, value)) if value == "last-write-wins" => Ok(ConflictPolicy::LastWriteWins),
        Some((_key, _value)) => Err(AppError::UnknownConflictPolicy),
    }
}

#[cfg(test)]
mod tests {
    use super::load_projection;

    #[test]
    fn projection_reads_saved_progress() {
        let dir = tempfile::tempdir().unwrap();
        let db = folio_storage::Database::open(&dir.path().join("folio.sqlite")).unwrap();
        db.set_progress("book-1", "epubcfi(/6/2)").unwrap();
        let projection = load_projection(&db).unwrap();
        assert_eq!(projection.reader.locator.as_deref(), Some("epubcfi(/6/2)"));
        assert!(projection.library.item_ids.is_empty());
        assert!(projection.servers.ids.is_empty());
        assert_eq!(
            projection.sync.policy(),
            folio_sync::ConflictPolicy::LastWriteWins
        );
    }
}
