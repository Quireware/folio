//! Identity and enrichment columns on `items`.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::StorageError;

/// One library row. Null size or modified time means the provider omitted it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogItem {
    /// Stable id. Desktop scans use the path relative to the library root.
    pub id: String,
    /// Content source. For a desktop file this is the same relative path as `id`.
    pub source: String,
    /// Filename or folder name without a book extension.
    pub label: String,
    /// `epub`, `cbz`, `cb7`, `cbt`, `cbr`, `pdf`, or `image-folder`.
    pub file_type: String,
    /// Byte length when the listing supplied one.
    pub size: Option<i64>,
    /// Modification time in Unix milliseconds when the listing supplied one.
    pub modified_ms: Option<i64>,
    /// Series guessed from the parent folder or the filename.
    pub series_guess: String,
    /// Series from ComicInfo or a sidecar, once enrichment has one.
    pub series: Option<String>,
    /// Display title. Pass 1 stores the label. Pass 2 may replace it.
    pub title: String,
    /// Page count after enrichment. Empty until then.
    pub page_count: Option<i64>,
    /// Image-folder child count from the listing. Empty for archives and PDFs.
    pub image_child_count: Option<i64>,
    /// Listing fingerprint this row was last written from.
    pub fingerprint: String,
    /// Fingerprint whose enrichment finished. Empty when pass 2 has not finished.
    pub enriched_fingerprint: Option<String>,
}

pub(crate) fn upsert(conn: &Connection, item: &CatalogItem) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO items (
            id, source, label, file_type, size, modified_ms, series_guess, series, title,
            page_count, image_child_count, fingerprint, enriched_fingerprint
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
        ON CONFLICT(id) DO UPDATE SET
            source = excluded.source,
            label = excluded.label,
            file_type = excluded.file_type,
            size = excluded.size,
            modified_ms = excluded.modified_ms,
            series_guess = excluded.series_guess,
            series = excluded.series,
            title = excluded.title,
            page_count = excluded.page_count,
            image_child_count = excluded.image_child_count,
            fingerprint = excluded.fingerprint,
            enriched_fingerprint = excluded.enriched_fingerprint",
        params![
            item.id,
            item.source,
            item.label,
            item.file_type,
            item.size,
            item.modified_ms,
            item.series_guess,
            item.series,
            item.title,
            item.page_count,
            item.image_child_count,
            item.fingerprint,
            item.enriched_fingerprint,
        ],
    )?;
    Ok(())
}

pub(crate) fn items(conn: &Connection) -> Result<Vec<CatalogItem>, StorageError> {
    let mut statement = conn.prepare(
        "SELECT id, source, COALESCE(label, title), file_type, size, modified_ms,
                COALESCE(series_guess, ''), series, title, page_count, image_child_count,
                COALESCE(fingerprint, ''), enriched_fingerprint
         FROM items ORDER BY id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(CatalogItem {
            id: row.get(0)?,
            source: row.get(1)?,
            label: row.get(2)?,
            file_type: row.get(3)?,
            size: row.get(4)?,
            modified_ms: row.get(5)?,
            series_guess: row.get(6)?,
            series: row.get(7)?,
            title: row.get(8)?,
            page_count: row.get(9)?,
            image_child_count: row.get(10)?,
            fingerprint: row.get(11)?,
            enriched_fingerprint: row.get(12)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::from)
}

pub(crate) fn delete_item(conn: &Connection, id: &str) -> Result<(), StorageError> {
    conn.execute("DELETE FROM items WHERE id = ?1", [id])?;
    Ok(())
}
