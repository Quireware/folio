//! Reading sessions. One row is one open of a book.

use rusqlite::{params, Connection};

use crate::StorageError;

/// A stored reading session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadingSession {
    /// Caller-chosen id.
    pub id: String,
    /// Book that was open.
    pub book_id: String,
    /// Start, Unix milliseconds.
    pub started_at: i64,
    /// End, Unix milliseconds, once the session is closed.
    pub ended_at: Option<i64>,
    /// Pages turned during the session.
    pub pages_read: i64,
}

pub(crate) fn start(
    conn: &Connection,
    id: &str,
    book_id: &str,
    started_at: i64,
) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO sessions (id, book_id, started_at, ended_at, pages_read)
         VALUES (?1, ?2, ?3, NULL, 0)",
        params![id, book_id, started_at],
    )?;
    Ok(())
}

pub(crate) fn finish(
    conn: &Connection,
    id: &str,
    ended_at: i64,
    pages_read: i64,
) -> Result<(), StorageError> {
    let changed = conn.execute(
        "UPDATE sessions SET ended_at = ?2, pages_read = ?3 WHERE id = ?1",
        params![id, ended_at, pages_read],
    )?;
    if changed != 1 {
        return Err(StorageError::Missing);
    }
    Ok(())
}

pub(crate) fn list(conn: &Connection) -> Result<Vec<ReadingSession>, StorageError> {
    let mut statement = conn.prepare(
        "SELECT id, book_id, started_at, ended_at, COALESCE(pages_read, 0)
         FROM sessions ORDER BY started_at, id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(ReadingSession {
            id: row.get(0)?,
            book_id: row.get(1)?,
            started_at: row.get(2)?,
            ended_at: row.get(3)?,
            pages_read: row.get(4)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::from)
}
