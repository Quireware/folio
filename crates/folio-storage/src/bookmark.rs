//! Bookmarks. The locator is a CFI or a page token.

use rusqlite::{params, Connection};

use crate::StorageError;

/// A named jump target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    /// Caller-chosen id.
    pub id: String,
    /// Book the target belongs to.
    pub book_id: String,
    /// CFI or `page:{n}`.
    pub locator: String,
    /// Name shown in the list.
    pub name: String,
}

pub(crate) fn insert(conn: &Connection, bookmark: &Bookmark) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO bookmarks (id, book_id, locator, name) VALUES (?1, ?2, ?3, ?4)",
        params![
            bookmark.id,
            bookmark.book_id,
            bookmark.locator,
            bookmark.name
        ],
    )?;
    Ok(())
}

pub(crate) fn rename(conn: &Connection, id: &str, name: &str) -> Result<(), StorageError> {
    let changed = conn.execute(
        "UPDATE bookmarks SET name = ?2 WHERE id = ?1",
        params![id, name],
    )?;
    if changed != 1 {
        return Err(StorageError::Missing);
    }
    Ok(())
}

pub(crate) fn delete(conn: &Connection, id: &str) -> Result<(), StorageError> {
    let changed = conn.execute("DELETE FROM bookmarks WHERE id = ?1", [id])?;
    if changed != 1 {
        return Err(StorageError::Missing);
    }
    Ok(())
}

pub(crate) fn list(conn: &Connection, book_id: &str) -> Result<Vec<Bookmark>, StorageError> {
    let mut statement = conn.prepare(
        "SELECT id, book_id, locator, COALESCE(name, '')
         FROM bookmarks WHERE book_id = ?1 ORDER BY id",
    )?;
    let rows = statement.query_map([book_id], |row| {
        Ok(Bookmark {
            id: row.get(0)?,
            book_id: row.get(1)?,
            locator: row.get(2)?,
            name: row.get(3)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::from)
}
