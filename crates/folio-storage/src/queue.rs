//! Download queue rows.

use rusqlite::{params, Connection};

use crate::{Database, StorageError};

/// One queued download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadRow {
    /// Caller-chosen id.
    pub id: String,
    /// Loopback URL.
    pub url: String,
    /// `queued`, `running`, `paused`, `cancelled`, or `done`.
    pub status: String,
}

/// A saved server. `secret_ref` is a secret-store key, never a raw password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredServer {
    /// Server id.
    pub id: String,
    /// Protocol name.
    pub kind: String,
    /// Base URL.
    pub url: String,
    /// Name shown in settings.
    pub display_name: String,
    /// Secret-store key.
    pub secret_ref: Option<String>,
}

/// Queue a URL once.
///
/// # Errors
///
/// Returns an error when the URL is already queued or the writer cannot commit.
pub fn enqueue_download(db: &Database, id: &str, url: &str) -> Result<(), StorageError> {
    let id = id.to_owned();
    let url = url.to_owned();
    if id.is_empty() || url.is_empty() {
        return Err(StorageError::EmptyId);
    }
    db.work(move |conn| {
        let existing: i64 = conn.query_row(
            "SELECT COUNT(*) FROM download_queue WHERE url = ?1",
            [&url],
            |row| row.get(0),
        )?;
        if existing > 0 {
            return Err(StorageError::Duplicate);
        }
        conn.execute(
            "INSERT INTO download_queue (id, url, status) VALUES (?1, ?2, 'queued')",
            params![id, url],
        )?;
        Ok(())
    })
}

/// Downloads in id order.
///
/// # Errors
///
/// Returns an error when the writer cannot read.
pub fn downloads(db: &Database) -> Result<Vec<DownloadRow>, StorageError> {
    db.work(list_downloads)
}

/// Set a download status.
///
/// # Errors
///
/// Returns an error when the row is missing or the writer cannot commit.
pub fn set_download_status(db: &Database, id: &str, status: &str) -> Result<(), StorageError> {
    let id = id.to_owned();
    let status = status.to_owned();
    db.work(move |conn| {
        let changed = conn.execute(
            "UPDATE download_queue SET status = ?2 WHERE id = ?1",
            params![id, status],
        )?;
        if changed != 1 {
            return Err(StorageError::Missing);
        }
        Ok(())
    })
}

/// Insert or replace a server row.
///
/// # Errors
///
/// Returns an error when an id or the URL is empty, or the writer cannot commit.
pub fn upsert_server(db: &Database, server: StoredServer) -> Result<(), StorageError> {
    if server.id.is_empty() || server.url.is_empty() {
        return Err(StorageError::EmptyId);
    }
    db.work(move |conn| upsert_server_row(conn, &server))
}

/// Remove a server row. The secret column is cleared before the row is deleted.
///
/// # Errors
///
/// Returns an error when the id is empty, the row is missing, or the writer cannot commit.
pub fn remove_server(db: &Database, id: &str) -> Result<(), StorageError> {
    if id.is_empty() {
        return Err(StorageError::EmptyId);
    }
    let id = id.to_owned();
    db.work(move |conn| delete_server_row(conn, &id))
}

/// Replace the row `old_id` with `server`, in one job, only while `old_id` still exists.
/// Returns `false`, writing nothing, when `old_id` is gone, so two callers moving the same row
/// cannot both succeed. When the ids differ and a row already has `server.id`, nothing is
/// written and [`StorageError::Taken`] is returned.
///
/// # Errors
///
/// Returns an error when an id or the URL is empty, the new id is taken, or the writer cannot
/// commit.
pub fn swap_server(
    db: &Database,
    old_id: &str,
    server: StoredServer,
) -> Result<bool, StorageError> {
    if old_id.is_empty() || server.id.is_empty() || server.url.is_empty() {
        return Err(StorageError::EmptyId);
    }
    let old_id = old_id.to_owned();
    db.work(move |conn| {
        let transaction = conn.transaction()?;
        if server_row(&transaction, &old_id)?.is_none() {
            return Ok(false);
        }
        if server.id != old_id {
            if server_row(&transaction, &server.id)?.is_some() {
                return Err(StorageError::Taken);
            }
            delete_server_row(&transaction, &old_id)?;
        }
        upsert_server_row(&transaction, &server)?;
        transaction.commit()?;
        Ok(true)
    })
}

pub(crate) fn upsert_server_row(
    conn: &Connection,
    server: &StoredServer,
) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO servers (id, kind, url, display_name, secret_ref)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET
            kind = excluded.kind,
            url = excluded.url,
            display_name = excluded.display_name,
            secret_ref = excluded.secret_ref",
        params![
            server.id,
            server.kind,
            server.url,
            server.display_name,
            server.secret_ref
        ],
    )?;
    Ok(())
}

pub(crate) fn delete_server_row(conn: &Connection, id: &str) -> Result<(), StorageError> {
    conn.execute("UPDATE servers SET secret_ref = NULL WHERE id = ?1", [id])?;
    let changed = conn.execute("DELETE FROM servers WHERE id = ?1", [id])?;
    if changed != 1 {
        return Err(StorageError::Missing);
    }
    Ok(())
}

pub(crate) fn server_row(
    conn: &Connection,
    id: &str,
) -> Result<Option<StoredServer>, StorageError> {
    let mut statement =
        conn.prepare("SELECT id, kind, url, display_name, secret_ref FROM servers WHERE id = ?1")?;
    let mut rows = statement.query([id])?;
    match rows.next()? {
        Some(row) => Ok(Some(StoredServer {
            id: row.get(0)?,
            kind: row.get(1)?,
            url: row.get(2)?,
            display_name: row.get(3)?,
            secret_ref: row.get(4)?,
        })),
        None => Ok(None),
    }
}

/// Server rows, in id order.
///
/// # Errors
///
/// Returns an error when the writer cannot read.
pub fn servers(db: &Database) -> Result<Vec<StoredServer>, StorageError> {
    db.work(|conn| {
        let mut statement = conn
            .prepare("SELECT id, kind, url, display_name, secret_ref FROM servers ORDER BY id")?;
        let rows = statement.query_map([], |row| {
            Ok(StoredServer {
                id: row.get(0)?,
                kind: row.get(1)?,
                url: row.get(2)?,
                display_name: row.get(3)?,
                secret_ref: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    })
}

fn list_downloads(conn: &mut Connection) -> Result<Vec<DownloadRow>, StorageError> {
    let mut statement =
        conn.prepare("SELECT id, url, COALESCE(status, 'queued') FROM download_queue ORDER BY id")?;
    let rows = statement.query_map([], |row| {
        Ok(DownloadRow {
            id: row.get(0)?,
            url: row.get(1)?,
            status: row.get(2)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::from)
}

#[cfg(test)]
mod tests {
    use super::{servers, swap_server, upsert_server, StoredServer};
    use crate::{Database, StorageError};

    fn row(id: &str, name: &str) -> StoredServer {
        StoredServer {
            id: id.to_owned(),
            kind: "opds".to_owned(),
            url: "https://books.test/opds".to_owned(),
            display_name: name.to_owned(),
            secret_ref: None,
        }
    }

    #[test]
    fn a_swap_moves_a_row_once_and_refuses_a_taken_id() {
        let dir = tempfile::tempdir().expect("dir");
        let db = Database::open(&dir.path().join("folio.sqlite")).expect("open");
        upsert_server(&db, row("Home", "Home")).expect("home");
        upsert_server(&db, row("Work", "Work")).expect("work");
        assert!(swap_server(&db, "Home", row("Attic", "Attic")).expect("swap"));
        assert!(!swap_server(&db, "Home", row("Cellar", "Cellar")).expect("gone"));
        assert!(matches!(
            swap_server(&db, "Attic", row("Work", "Work")),
            Err(StorageError::Taken)
        ));
        let ids = servers(&db)
            .expect("servers")
            .into_iter()
            .map(|server| server.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, ["Attic", "Work"]);
    }
}
