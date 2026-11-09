//! Annotation rows and the per-book search index.

use rusqlite::{params, Connection};

use crate::StorageError;

/// One annotation row. `body` is JSON the reader understands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAnnotation {
    /// Caller-chosen id.
    pub id: String,
    /// Book the annotation belongs to.
    pub book_id: String,
    /// CFI locator.
    pub locator: String,
    /// Colour and note, as JSON.
    pub body: String,
}

pub(crate) fn upsert_annotation(
    conn: &Connection,
    annotation: &StoredAnnotation,
) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO annotations (id, book_id, locator, body) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET
            book_id = excluded.book_id,
            locator = excluded.locator,
            body = excluded.body",
        params![
            annotation.id,
            annotation.book_id,
            annotation.locator,
            annotation.body
        ],
    )?;
    Ok(())
}

pub(crate) fn annotations(
    conn: &Connection,
    book_id: &str,
) -> Result<Vec<StoredAnnotation>, StorageError> {
    let mut statement = conn.prepare(
        "SELECT id, book_id, locator, body FROM annotations
         WHERE book_id = ?1 ORDER BY id",
    )?;
    let rows = statement.query_map([book_id], |row| {
        Ok(StoredAnnotation {
            id: row.get(0)?,
            book_id: row.get(1)?,
            locator: row.get(2)?,
            body: row.get(3)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::from)
}

pub(crate) fn delete_annotation(conn: &Connection, id: &str) -> Result<(), StorageError> {
    let changed = conn.execute("DELETE FROM annotations WHERE id = ?1", [id])?;
    if changed != 1 {
        return Err(StorageError::Missing);
    }
    Ok(())
}

pub(crate) fn replace_search(
    conn: &mut Connection,
    book_id: &str,
    chapters: &[(String, String)],
) -> Result<(), StorageError> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM search_index WHERE book_id = ?1", [book_id])?;
    for (chapter, body) in chapters {
        tx.execute(
            "INSERT INTO search_index (book_id, chapter, body) VALUES (?1, ?2, ?3)",
            params![book_id, chapter, body],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn search_chapters(
    conn: &Connection,
    book_id: &str,
) -> Result<Vec<(String, String)>, StorageError> {
    let mut statement =
        conn.prepare("SELECT chapter, body FROM search_index WHERE book_id = ?1 ORDER BY chapter")?;
    let rows = statement.query_map([book_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::from)
}
