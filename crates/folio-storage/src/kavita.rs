//! Kavita mirror, downloads, and progress outbox. These rows are not catalog items.

use std::collections::BTreeMap;

use rusqlite::{params, Connection};

use crate::queue::{delete_server_row, server_row, upsert_server_row, StoredServer};
use crate::{Database, StorageError};

/// One series visible for a published generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KavitaSeriesRow {
    /// Kavita series id.
    pub series_id: i64,
    /// Display name.
    pub name: String,
    /// `image`, `archive`, `epub`, `pdf`, or `unknown`.
    pub format: String,
    /// Owning library id.
    pub library_id: i64,
}

/// One library row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KavitaLibraryRow {
    /// Kavita library id.
    pub library_id: i64,
    /// Display name.
    pub name: String,
}

/// One chapter row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KavitaChapterRow {
    /// Kavita chapter id.
    pub chapter_id: i64,
    /// Series id.
    pub series_id: i64,
    /// Volume id.
    pub volume_id: i64,
    /// Chapter title.
    pub name: String,
    /// Page count.
    pub pages: i64,
}

/// One dashboard stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KavitaHomeRow {
    /// Stream id.
    pub stream_id: i64,
    /// Label.
    pub name: String,
    /// Kavita `DashboardStreamType`.
    pub stream_type: i64,
    /// Whether the user left the row visible.
    pub visible: bool,
    /// Sort order.
    pub sort_order: i64,
}

/// A named list row: collection, reading list, or smart filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KavitaNamedRow {
    /// Remote id.
    pub id: i64,
    /// Title or filter name.
    pub name: String,
    /// Encoded filter, empty for collections and reading lists.
    pub filter: String,
}

/// One bookmark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KavitaBookmarkRow {
    /// Bookmark id.
    pub bookmark_id: i64,
    /// Series id.
    pub series_id: i64,
    /// Chapter id.
    pub chapter_id: i64,
    /// Page index.
    pub page: i64,
}

/// One queued Kavita download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KavitaDownloadRow {
    /// Caller-chosen id.
    pub id: String,
    /// Server id.
    pub server_id: String,
    /// File URL on that server.
    pub url: String,
    /// `queued`, `running`, or `done`.
    pub status: String,
}

/// One progress push that has not been acknowledged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KavitaOutboxRow {
    /// Server id.
    pub server_id: String,
    /// Chapter id.
    pub chapter_id: i64,
    /// Series id.
    pub series_id: i64,
    /// Volume id.
    pub volume_id: i64,
    /// Library id.
    pub library_id: i64,
    /// `page:<n>`.
    pub locator: String,
    /// Chapter finished.
    pub complete: bool,
    /// Unix milliseconds.
    pub updated_at: i64,
}

/// The rows of one sync. [`publish_kavita_mirror`] stores them as one generation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KavitaMirror {
    /// Libraries.
    pub libraries: Vec<KavitaLibraryRow>,
    /// Every series.
    pub series: Vec<KavitaSeriesRow>,
    /// Dashboard streams.
    pub home: Vec<KavitaHomeRow>,
    /// Collections.
    pub collections: Vec<KavitaNamedRow>,
    /// Reading lists.
    pub reading_lists: Vec<KavitaNamedRow>,
    /// Smart filters.
    pub filters: Vec<KavitaNamedRow>,
    /// Want-to-read series ids.
    pub wants: Vec<i64>,
    /// Bookmarks.
    pub bookmarks: Vec<KavitaBookmarkRow>,
    /// Shelf name and its series ids, in shelf order.
    pub shelves: Vec<(String, Vec<i64>)>,
    /// Sections whose reply could not be read. Their published rows carry over, and any rows
    /// given for them above are ignored.
    pub keep: Vec<KavitaKeep>,
}

/// A section of the mirror that keeps its published rows through a publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KavitaKeep {
    /// Dashboard streams.
    Home,
    /// One shelf, by name.
    Shelf(String),
    /// Collections.
    Collections,
    /// Reading lists.
    ReadingLists,
    /// Smart filters.
    Filters,
    /// Want to Read.
    Wants,
    /// Bookmarks.
    Bookmarks,
}

/// Store `mirror` as the next generation and make it the visible one, in one transaction.
///
/// `current` is shown the saved server row first. When the row is gone or `current` returns
/// `false`, nothing is written and this returns `false`. Readers see the old generation or the
/// new one, never a mixture, because the whole publish is one job on the writer thread.
///
/// # Errors
///
/// Returns an error when the id or a name is empty, or the writer cannot commit.
pub fn publish_kavita_mirror<F>(
    db: &Database,
    server_id: &str,
    mirror: KavitaMirror,
    current: F,
) -> Result<bool, StorageError>
where
    F: FnOnce(&StoredServer) -> bool + Send + 'static,
{
    let server_id = owned_id(server_id)?;
    if mirror_has_empty_name(&mirror) {
        return Err(StorageError::EmptyId);
    }
    db.work(move |conn| {
        if !server_is_current(conn, &server_id, current)? {
            return Ok(false);
        }
        let transaction = conn.transaction()?;
        let published = published_generation(&transaction, &server_id)?;
        let generation = published.saturating_add(1);
        stage_mirror(&transaction, &server_id, published, generation, &mirror)?;
        publish_generation(&transaction, &server_id, generation)?;
        transaction.commit()?;
        Ok(true)
    })
}

/// Replace the chapters of `series_id` with `rows`, read when the series was opened.
///
/// The rows join the published generation, and [`publish_kavita_mirror`] keeps them while the
/// series stays in the mirror. `current` is checked as it is for a publish; when it refuses,
/// nothing is written and this returns `false`.
///
/// # Errors
///
/// Returns an error when the server id or a chapter name is empty, or the writer cannot commit.
pub fn replace_kavita_chapters<F>(
    db: &Database,
    server_id: &str,
    series_id: i64,
    rows: Vec<KavitaChapterRow>,
    current: F,
) -> Result<bool, StorageError>
where
    F: FnOnce(&StoredServer) -> bool + Send + 'static,
{
    let server_id = owned_id(server_id)?;
    if rows.iter().any(|row| row.name.is_empty()) {
        return Err(StorageError::EmptyId);
    }
    db.work(move |conn| {
        if !server_is_current(conn, &server_id, current)? {
            return Ok(false);
        }
        let published = published_generation(conn, &server_id)?;
        let transaction = conn.transaction()?;
        transaction.execute(
            "DELETE FROM kavita_chapter WHERE server_id = ?1 AND series_id = ?2",
            params![server_id, series_id],
        )?;
        for row in rows {
            transaction.execute(
                "INSERT INTO kavita_chapter
                    (server_id, chapter_id, series_id, volume_id, name, pages, generation)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(server_id, chapter_id) DO UPDATE SET
                    series_id = excluded.series_id,
                    volume_id = excluded.volume_id,
                    name = excluded.name,
                    pages = excluded.pages,
                    generation = excluded.generation",
                params![
                    server_id,
                    row.chapter_id,
                    row.series_id,
                    row.volume_id,
                    row.name,
                    row.pages,
                    published
                ],
            )?;
        }
        transaction.commit()?;
        Ok(true)
    })
}

fn server_is_current<F>(
    conn: &Connection,
    server_id: &str,
    current: F,
) -> Result<bool, StorageError>
where
    F: FnOnce(&StoredServer) -> bool,
{
    Ok(server_row(conn, server_id)?.is_some_and(|row| current(&row)))
}

fn mirror_has_empty_name(mirror: &KavitaMirror) -> bool {
    mirror.libraries.iter().any(|row| row.name.is_empty())
        || mirror.series.iter().any(|row| row.name.is_empty())
        || mirror.home.iter().any(|row| row.name.is_empty())
        || mirror
            .collections
            .iter()
            .chain(&mirror.reading_lists)
            .chain(&mirror.filters)
            .any(|row| row.name.is_empty())
        || mirror.shelves.iter().any(|(shelf, _)| shelf.is_empty())
}

fn stage_mirror(
    conn: &Connection,
    server_id: &str,
    published: i64,
    generation: i64,
    mirror: &KavitaMirror,
) -> Result<(), StorageError> {
    for keep in &mirror.keep {
        carry_forward(conn, server_id, published, generation, keep)?;
    }
    let kept = |section: &KavitaKeep| mirror.keep.contains(section);
    for row in &mirror.libraries {
        stage_library(conn, server_id, generation, row)?;
    }
    for row in &mirror.series {
        stage_series(conn, server_id, generation, row)?;
    }
    if !kept(&KavitaKeep::Home) {
        for row in &mirror.home {
            stage_home(conn, server_id, generation, row)?;
        }
    }
    if !kept(&KavitaKeep::Collections) {
        for row in &mirror.collections {
            stage_named(conn, COLLECTION_UPSERT, server_id, generation, row, false)?;
        }
    }
    if !kept(&KavitaKeep::ReadingLists) {
        for row in &mirror.reading_lists {
            stage_named(conn, READING_LIST_UPSERT, server_id, generation, row, false)?;
        }
    }
    if !kept(&KavitaKeep::Filters) {
        for row in &mirror.filters {
            stage_named(conn, FILTER_UPSERT, server_id, generation, row, true)?;
        }
    }
    if !kept(&KavitaKeep::Wants) {
        for series_id in mirror.wants.iter().copied() {
            stage_want(conn, server_id, generation, series_id)?;
        }
    }
    if !kept(&KavitaKeep::Bookmarks) {
        for row in &mirror.bookmarks {
            stage_bookmark(conn, server_id, generation, row)?;
        }
    }
    for (shelf, ids) in &mirror.shelves {
        let shelf_kept = mirror
            .keep
            .iter()
            .any(|keep| matches!(keep, KavitaKeep::Shelf(name) if name == shelf));
        if shelf_kept {
            continue;
        }
        for (position, series_id) in ids.iter().copied().enumerate() {
            let position = i64::try_from(position).unwrap_or(i64::MAX);
            stage_shelf(conn, server_id, generation, shelf, series_id, position)?;
        }
    }
    Ok(())
}

/// Move the published rows of one section to `generation`, so the publish keeps them.
fn carry_forward(
    conn: &Connection,
    server_id: &str,
    published: i64,
    generation: i64,
    keep: &KavitaKeep,
) -> Result<(), StorageError> {
    let table = match keep {
        KavitaKeep::Shelf(shelf) => {
            conn.execute(
                "UPDATE kavita_shelf SET generation = ?3
                 WHERE server_id = ?1 AND generation = ?2 AND shelf = ?4",
                params![server_id, published, generation, shelf],
            )?;
            return Ok(());
        }
        KavitaKeep::Home => "kavita_home",
        KavitaKeep::Collections => "kavita_collection",
        KavitaKeep::ReadingLists => "kavita_reading_list",
        KavitaKeep::Filters => "kavita_filter",
        KavitaKeep::Wants => "kavita_want",
        KavitaKeep::Bookmarks => "kavita_bookmark",
    };
    conn.execute(
        &format!("UPDATE {table} SET generation = ?3 WHERE server_id = ?1 AND generation = ?2"),
        params![server_id, published, generation],
    )?;
    Ok(())
}

fn stage_series(
    conn: &Connection,
    server_id: &str,
    generation: i64,
    row: &KavitaSeriesRow,
) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO kavita_series (server_id, series_id, name, format, library_id, generation)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(server_id, series_id) DO UPDATE SET
            name = excluded.name,
            format = excluded.format,
            library_id = excluded.library_id,
            generation = excluded.generation",
        params![
            server_id,
            row.series_id,
            row.name,
            row.format,
            row.library_id,
            generation
        ],
    )?;
    Ok(())
}

fn stage_library(
    conn: &Connection,
    server_id: &str,
    generation: i64,
    row: &KavitaLibraryRow,
) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO kavita_library (server_id, library_id, name, generation)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(server_id, library_id) DO UPDATE SET
            name = excluded.name,
            generation = excluded.generation",
        params![server_id, row.library_id, row.name, generation],
    )?;
    Ok(())
}

fn stage_home(
    conn: &Connection,
    server_id: &str,
    generation: i64,
    row: &KavitaHomeRow,
) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO kavita_home
            (server_id, stream_id, name, stream_type, visible, sort_order, generation)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(server_id, stream_id) DO UPDATE SET
            name = excluded.name,
            stream_type = excluded.stream_type,
            visible = excluded.visible,
            sort_order = excluded.sort_order,
            generation = excluded.generation",
        params![
            server_id,
            row.stream_id,
            row.name,
            row.stream_type,
            i64::from(row.visible),
            row.sort_order,
            generation
        ],
    )?;
    Ok(())
}

const COLLECTION_UPSERT: &str =
    "INSERT INTO kavita_collection (server_id, collection_id, title, generation)
     VALUES (?1, ?2, ?3, ?4)
     ON CONFLICT(server_id, collection_id) DO UPDATE SET
        title = excluded.title,
        generation = excluded.generation";

const READING_LIST_UPSERT: &str =
    "INSERT INTO kavita_reading_list (server_id, list_id, title, generation)
     VALUES (?1, ?2, ?3, ?4)
     ON CONFLICT(server_id, list_id) DO UPDATE SET
        title = excluded.title,
        generation = excluded.generation";

const FILTER_UPSERT: &str =
    "INSERT INTO kavita_filter (server_id, filter_id, name, filter, generation)
     VALUES (?1, ?2, ?3, ?4, ?5)
     ON CONFLICT(server_id, filter_id) DO UPDATE SET
        name = excluded.name,
        filter = excluded.filter,
        generation = excluded.generation";

fn stage_named(
    conn: &Connection,
    sql: &str,
    server_id: &str,
    generation: i64,
    row: &KavitaNamedRow,
    with_filter: bool,
) -> Result<(), StorageError> {
    if with_filter {
        conn.execute(
            sql,
            params![server_id, row.id, row.name, row.filter, generation],
        )?;
    } else {
        conn.execute(sql, params![server_id, row.id, row.name, generation])?;
    }
    Ok(())
}

fn stage_want(
    conn: &Connection,
    server_id: &str,
    generation: i64,
    series_id: i64,
) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO kavita_want (server_id, series_id, generation)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(server_id, series_id) DO UPDATE SET generation = excluded.generation",
        params![server_id, series_id, generation],
    )?;
    Ok(())
}

fn stage_bookmark(
    conn: &Connection,
    server_id: &str,
    generation: i64,
    row: &KavitaBookmarkRow,
) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO kavita_bookmark
            (server_id, bookmark_id, series_id, chapter_id, page, generation)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(server_id, bookmark_id) DO UPDATE SET
            series_id = excluded.series_id,
            chapter_id = excluded.chapter_id,
            page = excluded.page,
            generation = excluded.generation",
        params![
            server_id,
            row.bookmark_id,
            row.series_id,
            row.chapter_id,
            row.page,
            generation
        ],
    )?;
    Ok(())
}

fn stage_shelf(
    conn: &Connection,
    server_id: &str,
    generation: i64,
    shelf: &str,
    series_id: i64,
    position: i64,
) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO kavita_shelf (server_id, shelf, series_id, position, generation)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(server_id, shelf, series_id) DO UPDATE SET
            position = excluded.position,
            generation = excluded.generation",
        params![server_id, shelf, series_id, position, generation],
    )?;
    Ok(())
}

/// Make `generation` the visible one and drop every other generation for `server_id`.
fn publish_generation(
    conn: &Connection,
    server_id: &str,
    generation: i64,
) -> Result<(), StorageError> {
    // Chapters are read when a series is opened, not staged by a sync, so the ones whose
    // series is still in the mirror move to the new generation before old rows go.
    conn.execute(
        "UPDATE kavita_chapter SET generation = ?2
         WHERE server_id = ?1 AND generation != ?2 AND series_id IN
            (SELECT series_id FROM kavita_series WHERE server_id = ?1 AND generation = ?2)",
        params![server_id, generation],
    )?;
    for table in GENERATION_TABLES {
        conn.execute(
            &format!("DELETE FROM {table} WHERE server_id = ?1 AND generation != ?2"),
            params![server_id, generation],
        )?;
    }
    conn.execute(
        "INSERT INTO kavita_generation (server_id, published) VALUES (?1, ?2)
         ON CONFLICT(server_id) DO UPDATE SET published = excluded.published",
        params![server_id, generation],
    )?;
    Ok(())
}

/// Everything the Kavita screen draws for one server, read in one job so that every list comes
/// from the same published generation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KavitaVisible {
    /// Libraries, ordered by name.
    pub libraries: Vec<KavitaLibraryRow>,
    /// Series, ordered by name.
    pub series: Vec<KavitaSeriesRow>,
    /// Dashboard streams, in sort order.
    pub home: Vec<KavitaHomeRow>,
    /// Want-to-read series ids.
    pub wants: Vec<i64>,
    /// Bookmarks.
    pub bookmarks: Vec<KavitaBookmarkRow>,
    /// Shelf name to its series ids, in shelf order.
    pub shelves: BTreeMap<String, Vec<i64>>,
}

/// The published mirror of `server_id`.
///
/// # Errors
///
/// Returns an error when the id is empty or the writer cannot read.
pub fn visible_kavita_mirror(
    db: &Database,
    server_id: &str,
) -> Result<KavitaVisible, StorageError> {
    let server_id = owned_id(server_id)?;
    db.work(move |conn| {
        let published = published_generation(conn, &server_id)?;
        Ok(KavitaVisible {
            libraries: libraries_at(conn, &server_id, published)?,
            series: series_at(conn, &server_id, published)?,
            home: home_at(conn, &server_id, published)?,
            wants: wants_at(conn, &server_id, published)?,
            bookmarks: bookmarks_at(conn, &server_id, published)?,
            shelves: shelves_at(conn, &server_id, published)?,
        })
    })
}

/// Series for the published generation, ordered by name.
///
/// # Errors
///
/// Returns an error when the writer cannot read.
pub fn visible_kavita_series(
    db: &Database,
    server_id: &str,
) -> Result<Vec<KavitaSeriesRow>, StorageError> {
    let server_id = owned_id(server_id)?;
    db.work(move |conn| {
        let published = published_generation(conn, &server_id)?;
        series_at(conn, &server_id, published)
    })
}

/// Published chapters for one series.
///
/// # Errors
///
/// Returns an error when the writer cannot read.
pub fn visible_kavita_chapters(
    db: &Database,
    server_id: &str,
    series_id: i64,
) -> Result<Vec<KavitaChapterRow>, StorageError> {
    let server_id = owned_id(server_id)?;
    db.work(move |conn| {
        let published = published_generation(conn, &server_id)?;
        let mut statement = conn.prepare(
            "SELECT chapter_id, series_id, volume_id, name, pages FROM kavita_chapter
             WHERE server_id = ?1 AND series_id = ?2 AND generation = ?3
             ORDER BY volume_id, chapter_id",
        )?;
        let rows = statement.query_map(params![server_id, series_id, published], |row| {
            Ok(KavitaChapterRow {
                chapter_id: row.get(0)?,
                series_id: row.get(1)?,
                volume_id: row.get(2)?,
                name: row.get(3)?,
                pages: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    })
}

/// Published collections.
///
/// # Errors
///
/// Returns an error when the writer cannot read.
pub fn visible_kavita_collections(
    db: &Database,
    server_id: &str,
) -> Result<Vec<KavitaNamedRow>, StorageError> {
    visible_named(
        db,
        server_id,
        "SELECT collection_id, title, '' FROM kavita_collection
         WHERE server_id = ?1 AND generation = ?2 ORDER BY title",
    )
}

/// Published reading lists.
///
/// # Errors
///
/// Returns an error when the writer cannot read.
pub fn visible_kavita_reading_lists(
    db: &Database,
    server_id: &str,
) -> Result<Vec<KavitaNamedRow>, StorageError> {
    visible_named(
        db,
        server_id,
        "SELECT list_id, title, '' FROM kavita_reading_list
         WHERE server_id = ?1 AND generation = ?2 ORDER BY title",
    )
}

/// Published smart filters.
///
/// # Errors
///
/// Returns an error when the writer cannot read.
pub fn visible_kavita_filters(
    db: &Database,
    server_id: &str,
) -> Result<Vec<KavitaNamedRow>, StorageError> {
    visible_named(
        db,
        server_id,
        "SELECT filter_id, name, filter FROM kavita_filter
         WHERE server_id = ?1 AND generation = ?2 ORDER BY name",
    )
}

fn libraries_at(
    conn: &Connection,
    server_id: &str,
    published: i64,
) -> Result<Vec<KavitaLibraryRow>, StorageError> {
    let mut statement = conn.prepare(
        "SELECT library_id, name FROM kavita_library
         WHERE server_id = ?1 AND generation = ?2 ORDER BY name",
    )?;
    let rows = statement.query_map(params![server_id, published], |row| {
        Ok(KavitaLibraryRow {
            library_id: row.get(0)?,
            name: row.get(1)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::from)
}

fn series_at(
    conn: &Connection,
    server_id: &str,
    published: i64,
) -> Result<Vec<KavitaSeriesRow>, StorageError> {
    let mut statement = conn.prepare(
        "SELECT series_id, name, format, library_id FROM kavita_series
         WHERE server_id = ?1 AND generation = ?2 ORDER BY name",
    )?;
    let rows = statement.query_map(params![server_id, published], |row| {
        Ok(KavitaSeriesRow {
            series_id: row.get(0)?,
            name: row.get(1)?,
            format: row.get(2)?,
            library_id: row.get(3)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::from)
}

fn home_at(
    conn: &Connection,
    server_id: &str,
    published: i64,
) -> Result<Vec<KavitaHomeRow>, StorageError> {
    let mut statement = conn.prepare(
        "SELECT stream_id, name, stream_type, visible, sort_order FROM kavita_home
         WHERE server_id = ?1 AND generation = ?2 ORDER BY sort_order",
    )?;
    let rows = statement.query_map(params![server_id, published], |row| {
        let visible: i64 = row.get(3)?;
        Ok(KavitaHomeRow {
            stream_id: row.get(0)?,
            name: row.get(1)?,
            stream_type: row.get(2)?,
            visible: visible != 0,
            sort_order: row.get(4)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::from)
}

fn wants_at(conn: &Connection, server_id: &str, published: i64) -> Result<Vec<i64>, StorageError> {
    let mut statement = conn.prepare(
        "SELECT series_id FROM kavita_want
         WHERE server_id = ?1 AND generation = ?2 ORDER BY series_id",
    )?;
    let rows = statement.query_map(params![server_id, published], |row| row.get(0))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::from)
}

fn bookmarks_at(
    conn: &Connection,
    server_id: &str,
    published: i64,
) -> Result<Vec<KavitaBookmarkRow>, StorageError> {
    let mut statement = conn.prepare(
        "SELECT bookmark_id, series_id, chapter_id, page FROM kavita_bookmark
         WHERE server_id = ?1 AND generation = ?2 ORDER BY bookmark_id",
    )?;
    let rows = statement.query_map(params![server_id, published], |row| {
        Ok(KavitaBookmarkRow {
            bookmark_id: row.get(0)?,
            series_id: row.get(1)?,
            chapter_id: row.get(2)?,
            page: row.get(3)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::from)
}

fn shelves_at(
    conn: &Connection,
    server_id: &str,
    published: i64,
) -> Result<BTreeMap<String, Vec<i64>>, StorageError> {
    let mut statement = conn.prepare(
        "SELECT shelf, series_id FROM kavita_shelf
         WHERE server_id = ?1 AND generation = ?2
         ORDER BY shelf, position",
    )?;
    let rows = statement.query_map(params![server_id, published], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut shelves = BTreeMap::<String, Vec<i64>>::new();
    for row in rows {
        let (shelf, series_id) = row?;
        shelves.entry(shelf).or_default().push(series_id);
    }
    Ok(shelves)
}

/// Drop the mirror, downloads and outbox of `server.id` and store `server`, in one job, so no
/// publish can land between the two.
///
/// # Errors
///
/// Returns an error when an id or the URL is empty, or the writer cannot commit.
pub fn reset_kavita_server(db: &Database, server: StoredServer) -> Result<(), StorageError> {
    if server.id.is_empty() || server.url.is_empty() {
        return Err(StorageError::EmptyId);
    }
    db.work(move |conn| {
        let transaction = conn.transaction()?;
        delete_server_tables(&transaction, &server.id)?;
        upsert_server_row(&transaction, &server)?;
        transaction.commit()?;
        Ok(())
    })
}

/// Drop the mirror, downloads and outbox of `server_id` and its server row, in one job.
///
/// # Errors
///
/// Returns [`StorageError::Missing`] when the row does not exist, or an error when the id is
/// empty or the writer cannot commit.
pub fn forget_kavita_server(db: &Database, server_id: &str) -> Result<(), StorageError> {
    let server_id = owned_id(server_id)?;
    db.work(move |conn| {
        let transaction = conn.transaction()?;
        delete_server_tables(&transaction, &server_id)?;
        delete_server_row(&transaction, &server_id)?;
        transaction.commit()?;
        Ok(())
    })
}

fn delete_server_tables(conn: &Connection, server_id: &str) -> Result<(), StorageError> {
    for table in ALL_SERVER_TABLES {
        conn.execute(
            &format!("DELETE FROM {table} WHERE server_id = ?1"),
            [server_id],
        )?;
    }
    Ok(())
}

/// Queue a download. A duplicate id goes back to `queued` unless it is running.
///
/// # Errors
///
/// Returns an error when the id or URL is empty.
pub fn enqueue_kavita_download(
    db: &Database,
    id: &str,
    server_id: &str,
    url: &str,
) -> Result<(), StorageError> {
    if id.is_empty() || url.is_empty() {
        return Err(StorageError::EmptyId);
    }
    let id = id.to_owned();
    let server_id = owned_id(server_id)?;
    let url = url.to_owned();
    db.work(move |conn| {
        conn.execute(
            "INSERT INTO kavita_download (id, server_id, url, status) VALUES (?1, ?2, ?3, 'queued')
             ON CONFLICT(id) DO UPDATE SET url = excluded.url, status = 'queued'
             WHERE kavita_download.status != 'running'",
            params![id, server_id, url],
        )?;
        Ok(())
    })
}

/// Status of one download.
///
/// # Errors
///
/// Returns an error when the writer cannot read.
pub fn kavita_download_status(db: &Database, id: &str) -> Result<Option<String>, StorageError> {
    let id = id.to_owned();
    db.work(move |conn| {
        let mut statement = conn.prepare("SELECT status FROM kavita_download WHERE id = ?1")?;
        let mut rows = statement.query([&id])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    })
}

/// Move one download from `queued` to `running`. Returns `false` when it was not queued, so
/// two loops never fetch the same chapter.
///
/// # Errors
///
/// Returns an error when the writer cannot commit.
pub fn claim_kavita_download(db: &Database, id: &str) -> Result<bool, StorageError> {
    let id = id.to_owned();
    db.work(move |conn| {
        let changed = conn.execute(
            "UPDATE kavita_download SET status = 'running' WHERE id = ?1 AND status = 'queued'",
            [&id],
        )?;
        Ok(changed == 1)
    })
}

/// Move one download from `running` to `status`. Returns `false` when it was no longer
/// running: something cancelled or removed it while it ran.
///
/// # Errors
///
/// Returns an error when the writer cannot commit.
pub fn settle_kavita_download(db: &Database, id: &str, status: &str) -> Result<bool, StorageError> {
    let id = id.to_owned();
    let status = status.to_owned();
    db.work(move |conn| {
        let changed = conn.execute(
            "UPDATE kavita_download SET status = ?2 WHERE id = ?1 AND status = 'running'",
            params![id, status],
        )?;
        Ok(changed == 1)
    })
}

/// Downloads in a given status.
///
/// # Errors
///
/// Returns an error when the writer cannot read.
pub fn kavita_downloads_by_status(
    db: &Database,
    status: &str,
) -> Result<Vec<KavitaDownloadRow>, StorageError> {
    let status = status.to_owned();
    db.work(move |conn| {
        let mut statement = conn.prepare(
            "SELECT id, server_id, url, status FROM kavita_download WHERE status = ?1 ORDER BY id",
        )?;
        let rows = statement.query_map([&status], |row| {
            Ok(KavitaDownloadRow {
                id: row.get(0)?,
                server_id: row.get(1)?,
                url: row.get(2)?,
                status: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    })
}

/// Every download row, including `running`.
///
/// # Errors
///
/// Returns an error when the writer cannot read.
pub fn kavita_downloads(db: &Database) -> Result<Vec<KavitaDownloadRow>, StorageError> {
    db.work(|conn| {
        let mut statement =
            conn.prepare("SELECT id, server_id, url, status FROM kavita_download ORDER BY id")?;
        let rows = statement.query_map([], |row| {
            Ok(KavitaDownloadRow {
                id: row.get(0)?,
                server_id: row.get(1)?,
                url: row.get(2)?,
                status: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    })
}

/// Insert or replace an outbox row.
///
/// # Errors
///
/// Returns an error when the server id or locator is empty.
pub fn enqueue_kavita_progress(db: &Database, row: KavitaOutboxRow) -> Result<(), StorageError> {
    if row.server_id.is_empty() || row.locator.is_empty() {
        return Err(StorageError::EmptyId);
    }
    db.work(move |conn| {
        let complete = i64::from(row.complete);
        conn.execute(
            "INSERT INTO kavita_outbox
                (server_id, chapter_id, series_id, volume_id, library_id, locator, complete, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(server_id, chapter_id) DO UPDATE SET
                series_id = excluded.series_id,
                volume_id = excluded.volume_id,
                library_id = excluded.library_id,
                locator = excluded.locator,
                complete = excluded.complete,
                updated_at = excluded.updated_at
             WHERE excluded.updated_at >= kavita_outbox.updated_at",
            params![
                row.server_id,
                row.chapter_id,
                row.series_id,
                row.volume_id,
                row.library_id,
                row.locator,
                complete,
                row.updated_at
            ],
        )?;
        Ok(())
    })
}

/// Pending progress for one chapter.
///
/// # Errors
///
/// Returns an error when the writer cannot read.
pub fn kavita_pending(
    db: &Database,
    server_id: &str,
    chapter_id: i64,
) -> Result<Option<KavitaOutboxRow>, StorageError> {
    let server_id = owned_id(server_id)?;
    db.work(move |conn| read_outbox(conn, &server_id, chapter_id))
}

/// Every pending row for a server.
///
/// # Errors
///
/// Returns an error when the writer cannot read.
pub fn kavita_outbox(db: &Database, server_id: &str) -> Result<Vec<KavitaOutboxRow>, StorageError> {
    let server_id = owned_id(server_id)?;
    db.work(move |conn| {
        let mut statement = conn.prepare(
            "SELECT server_id, chapter_id, series_id, volume_id, library_id, locator, complete, updated_at
             FROM kavita_outbox WHERE server_id = ?1 ORDER BY chapter_id",
        )?;
        let rows = statement.query_map([&server_id], outbox_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(StorageError::from)
    })
}

/// Remove `sent` once the server has it. A row that changed since it was read (a later page
/// in the same chapter) stays for the next push.
///
/// # Errors
///
/// Returns an error when the server id is empty or the writer cannot commit.
pub fn acknowledge_kavita_progress(
    db: &Database,
    sent: &KavitaOutboxRow,
) -> Result<(), StorageError> {
    let server_id = owned_id(&sent.server_id)?;
    let chapter_id = sent.chapter_id;
    let updated_at = sent.updated_at;
    // clone: the writer thread compares the locator after this call returns
    let locator = sent.locator.clone();
    db.work(move |conn| {
        conn.execute(
            "DELETE FROM kavita_outbox
             WHERE server_id = ?1 AND chapter_id = ?2 AND updated_at = ?3 AND locator = ?4",
            params![server_id, chapter_id, updated_at, locator],
        )?;
        Ok(())
    })
}

/// Store a pulled locator only when that chapter has no outbox row.
///
/// # Errors
///
/// Returns an error when the writer cannot commit.
pub fn apply_kavita_pull(
    db: &Database,
    server_id: &str,
    chapter_id: i64,
    locator: &str,
    updated_at: i64,
) -> Result<(), StorageError> {
    let server_id = owned_id(server_id)?;
    let locator = locator.to_owned();
    db.work(move |conn| {
        if read_outbox(conn, &server_id, chapter_id)?.is_some() {
            return Ok(());
        }
        conn.execute(
            "INSERT INTO kavita_progress (server_id, chapter_id, locator, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(server_id, chapter_id) DO UPDATE SET
                locator = excluded.locator,
                updated_at = excluded.updated_at
             WHERE excluded.updated_at >= kavita_progress.updated_at",
            params![server_id, chapter_id, locator, updated_at],
        )?;
        Ok(())
    })
}

/// A read or unread mark that has not been acknowledged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KavitaMarkRow {
    /// Server id.
    pub server_id: String,
    /// `chapter`, `volume`, or `series`.
    pub scope: String,
    /// Id of the chapter, volume, or series.
    pub scope_id: i64,
    /// Series id.
    pub series_id: i64,
    /// Volume id. Zero when the scope is the series.
    pub volume_id: i64,
    /// `true` marks read.
    pub read: bool,
    /// Unix milliseconds.
    pub updated_at: i64,
}

/// Insert or replace a mark. An older timestamp does not replace a newer one.
///
/// # Errors
///
/// Returns an error when the scope is not chapter, volume, or series.
pub fn enqueue_kavita_mark(db: &Database, row: KavitaMarkRow) -> Result<(), StorageError> {
    if !matches!(row.scope.as_str(), "chapter" | "volume" | "series") || row.server_id.is_empty() {
        return Err(StorageError::EmptyId);
    }
    db.work(move |conn| {
        let read = i64::from(row.read);
        conn.execute(
            "INSERT INTO kavita_mark
                (server_id, scope, scope_id, series_id, volume_id, read, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(server_id, scope, scope_id) DO UPDATE SET
                series_id = excluded.series_id,
                volume_id = excluded.volume_id,
                read = excluded.read,
                updated_at = excluded.updated_at
             WHERE excluded.updated_at >= kavita_mark.updated_at",
            params![
                row.server_id,
                row.scope,
                row.scope_id,
                row.series_id,
                row.volume_id,
                read,
                row.updated_at
            ],
        )?;
        Ok(())
    })
}

/// Pending marks for one server.
///
/// # Errors
///
/// Returns an error when the writer cannot read.
pub fn kavita_marks(db: &Database, server_id: &str) -> Result<Vec<KavitaMarkRow>, StorageError> {
    let server_id = owned_id(server_id)?;
    db.work(move |conn| {
        let mut statement = conn.prepare(
            "SELECT server_id, scope, scope_id, series_id, volume_id, read, updated_at
             FROM kavita_mark WHERE server_id = ?1 ORDER BY scope, scope_id",
        )?;
        let rows = statement.query_map([&server_id], |row| {
            let read: i64 = row.get(5)?;
            Ok(KavitaMarkRow {
                server_id: row.get(0)?,
                scope: row.get(1)?,
                scope_id: row.get(2)?,
                series_id: row.get(3)?,
                volume_id: row.get(4)?,
                read: read != 0,
                updated_at: row.get(6)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    })
}

/// Remove `sent` once the server has it. A mark that changed since it was read stays for the
/// next push.
///
/// # Errors
///
/// Returns an error when the server id is empty or the writer cannot commit.
pub fn acknowledge_kavita_mark(db: &Database, sent: &KavitaMarkRow) -> Result<(), StorageError> {
    let server_id = owned_id(&sent.server_id)?;
    // clone: the writer thread deletes the row after this call returns
    let scope = sent.scope.clone();
    let scope_id = sent.scope_id;
    let updated_at = sent.updated_at;
    let read = i64::from(sent.read);
    db.work(move |conn| {
        conn.execute(
            "DELETE FROM kavita_mark
             WHERE server_id = ?1 AND scope = ?2 AND scope_id = ?3 AND updated_at = ?4
                AND read = ?5",
            params![server_id, scope, scope_id, updated_at, read],
        )?;
        Ok(())
    })
}

const GENERATION_TABLES: [&str; 10] = [
    "kavita_series",
    "kavita_library",
    "kavita_chapter",
    "kavita_home",
    "kavita_collection",
    "kavita_reading_list",
    "kavita_filter",
    "kavita_want",
    "kavita_bookmark",
    "kavita_shelf",
];

const ALL_SERVER_TABLES: [&str; 15] = [
    "kavita_generation",
    "kavita_series",
    "kavita_library",
    "kavita_chapter",
    "kavita_home",
    "kavita_collection",
    "kavita_reading_list",
    "kavita_filter",
    "kavita_want",
    "kavita_bookmark",
    "kavita_shelf",
    "kavita_download",
    "kavita_outbox",
    "kavita_progress",
    "kavita_mark",
];

fn visible_named(
    db: &Database,
    server_id: &str,
    sql: &'static str,
) -> Result<Vec<KavitaNamedRow>, StorageError> {
    let server_id = owned_id(server_id)?;
    db.work(move |conn| {
        let published = published_generation(conn, &server_id)?;
        let mut statement = conn.prepare(sql)?;
        let rows = statement.query_map(params![server_id, published], |row| {
            Ok(KavitaNamedRow {
                id: row.get(0)?,
                name: row.get(1)?,
                filter: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    })
}

fn published_generation(conn: &Connection, server_id: &str) -> Result<i64, StorageError> {
    let mut statement =
        conn.prepare("SELECT published FROM kavita_generation WHERE server_id = ?1")?;
    let mut rows = statement.query([server_id])?;
    match rows.next()? {
        Some(row) => Ok(row.get(0)?),
        None => Ok(0),
    }
}

fn read_outbox(
    conn: &Connection,
    server_id: &str,
    chapter_id: i64,
) -> Result<Option<KavitaOutboxRow>, StorageError> {
    let mut statement = conn.prepare(
        "SELECT server_id, chapter_id, series_id, volume_id, library_id, locator, complete, updated_at
         FROM kavita_outbox WHERE server_id = ?1 AND chapter_id = ?2",
    )?;
    let mut rows = statement.query(params![server_id, chapter_id])?;
    match rows.next()? {
        Some(row) => Ok(Some(outbox_row(row)?)),
        None => Ok(None),
    }
}

fn outbox_row(row: &rusqlite::Row<'_>) -> Result<KavitaOutboxRow, rusqlite::Error> {
    let complete: i64 = row.get(6)?;
    Ok(KavitaOutboxRow {
        server_id: row.get(0)?,
        chapter_id: row.get(1)?,
        series_id: row.get(2)?,
        volume_id: row.get(3)?,
        library_id: row.get(4)?,
        locator: row.get(5)?,
        complete: complete != 0,
        updated_at: row.get(7)?,
    })
}

fn owned_id(server_id: &str) -> Result<String, StorageError> {
    if server_id.is_empty() {
        Err(StorageError::EmptyId)
    } else {
        Ok(server_id.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        acknowledge_kavita_mark, acknowledge_kavita_progress, apply_kavita_pull,
        claim_kavita_download, enqueue_kavita_download, enqueue_kavita_mark,
        enqueue_kavita_progress, forget_kavita_server, kavita_download_status, kavita_marks,
        kavita_pending, publish_kavita_mirror, replace_kavita_chapters, reset_kavita_server,
        settle_kavita_download, visible_kavita_chapters, visible_kavita_mirror,
        visible_kavita_series, KavitaChapterRow, KavitaHomeRow, KavitaKeep, KavitaLibraryRow,
        KavitaMarkRow, KavitaMirror, KavitaOutboxRow, KavitaSeriesRow,
    };
    use crate::catalog;
    use crate::queue::{servers, upsert_server, StoredServer};
    use crate::Database;

    fn test_db() -> Database {
        let path = std::env::temp_dir().join(format!(
            "folio-kavita-{}-{}.sqlite",
            std::process::id(),
            unique_name()
        ));
        if path.exists() {
            std::fs::remove_file(&path).expect("remove");
        }
        let db = Database::open(&path).expect("open");
        for id in ["server-1", "server-2"] {
            upsert_server(&db, stored(id, "http://kavita.test")).expect("server");
        }
        db
    }

    fn stored(id: &str, url: &str) -> StoredServer {
        StoredServer {
            id: id.to_owned(),
            kind: "kavita".to_owned(),
            url: url.to_owned(),
            display_name: "Home".to_owned(),
            secret_ref: None,
        }
    }

    fn unique_name() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        format!("{nanos}")
    }

    fn series(id: i64, name: &str) -> KavitaSeriesRow {
        KavitaSeriesRow {
            series_id: id,
            name: name.to_owned(),
            format: "epub".to_owned(),
            library_id: 1,
        }
    }

    fn chapter(id: i64, series_id: i64) -> KavitaChapterRow {
        KavitaChapterRow {
            chapter_id: id,
            series_id,
            volume_id: 1,
            name: format!("Chapter {id}"),
            pages: 10,
        }
    }

    fn home(id: i64) -> KavitaHomeRow {
        KavitaHomeRow {
            stream_id: id,
            name: format!("Stream {id}"),
            stream_type: 1,
            visible: true,
            sort_order: id,
        }
    }

    fn with_series(rows: &[(i64, &str)]) -> KavitaMirror {
        KavitaMirror {
            series: rows.iter().map(|(id, name)| series(*id, name)).collect(),
            ..KavitaMirror::default()
        }
    }

    fn publish(db: &Database, server_id: &str, mirror: KavitaMirror) -> bool {
        publish_kavita_mirror(db, server_id, mirror, |_| true).expect("publish")
    }

    fn chapters(db: &Database, series_id: i64, rows: Vec<KavitaChapterRow>) -> bool {
        replace_kavita_chapters(db, "server-1", series_id, rows, |_| true).expect("chapters")
    }

    fn chapter_ids(db: &Database, series_id: i64) -> Vec<i64> {
        visible_kavita_chapters(db, "server-1", series_id)
            .expect("chapters")
            .iter()
            .map(|row| row.chapter_id)
            .collect()
    }

    fn series_names(db: &Database, server_id: &str) -> Vec<String> {
        visible_kavita_series(db, server_id)
            .expect("visible")
            .into_iter()
            .map(|row| row.name)
            .collect()
    }

    fn outbox(updated_at: i64, locator: &str) -> KavitaOutboxRow {
        KavitaOutboxRow {
            server_id: "server-1".to_owned(),
            chapter_id: 9,
            series_id: 1,
            volume_id: 1,
            library_id: 1,
            locator: locator.to_owned(),
            complete: false,
            updated_at,
        }
    }

    fn mark(updated_at: i64, read: bool) -> KavitaMarkRow {
        KavitaMarkRow {
            server_id: "server-1".to_owned(),
            scope: "chapter".to_owned(),
            scope_id: 9,
            series_id: 1,
            volume_id: 1,
            read,
            updated_at,
        }
    }

    #[test]
    fn opened_chapters_survive_a_sync_while_their_series_stays() {
        let db = test_db();
        assert!(publish(
            &db,
            "server-1",
            with_series(&[(1, "Wings"), (2, "Tide")])
        ));
        assert!(chapters(&db, 1, vec![chapter(10, 1), chapter(11, 1)]));
        assert!(chapters(&db, 2, vec![chapter(20, 2)]));
        assert_eq!(chapter_ids(&db, 1), [10, 11]);
        assert!(chapters(&db, 1, vec![chapter(12, 1)]));
        assert_eq!(chapter_ids(&db, 1), [12]);
        assert_eq!(chapter_ids(&db, 2), [20]);
        assert!(publish(&db, "server-1", with_series(&[(1, "Wings")])));
        assert_eq!(chapter_ids(&db, 1), [12]);
        assert!(chapter_ids(&db, 2).is_empty());
        assert!(publish(&db, "server-1", with_series(&[(2, "Tide")])));
        assert!(chapter_ids(&db, 2).is_empty());
    }

    #[test]
    fn a_refused_check_or_a_missing_server_writes_nothing() {
        let db = test_db();
        let first = KavitaMirror {
            libraries: vec![KavitaLibraryRow {
                library_id: 4,
                name: "Books".to_owned(),
            }],
            ..with_series(&[(1, "Old")])
        };
        assert!(publish(&db, "server-1", first));
        let refused = publish_kavita_mirror(&db, "server-1", with_series(&[(2, "New")]), |row| {
            row.url == "http://elsewhere.test"
        })
        .expect("refused");
        assert!(!refused);
        assert_eq!(series_names(&db, "server-1"), ["Old"]);
        let shown = visible_kavita_mirror(&db, "server-1").expect("mirror");
        assert_eq!(shown.libraries.len(), 1);
        assert!(!publish(&db, "server-9", with_series(&[(3, "Nowhere")])));
        assert!(series_names(&db, "server-9").is_empty());
        let stored = replace_kavita_chapters(&db, "server-1", 1, vec![chapter(10, 1)], |_| false)
            .expect("chapters");
        assert!(!stored);
        assert!(chapter_ids(&db, 1).is_empty());
        assert!(publish(&db, "server-2", with_series(&[(9, "Elsewhere")])));
        assert_eq!(series_names(&db, "server-1"), ["Old"]);
    }

    #[test]
    fn stale_sections_keep_their_published_rows() {
        let db = test_db();
        let first = KavitaMirror {
            home: vec![home(1)],
            wants: vec![1],
            shelves: vec![
                ("on-deck".to_owned(), vec![1, 2]),
                ("added".to_owned(), vec![2]),
            ],
            ..with_series(&[(1, "Wings"), (2, "Tide")])
        };
        assert!(publish(&db, "server-1", first));
        let second = KavitaMirror {
            home: vec![home(7)],
            wants: vec![2],
            shelves: vec![
                ("on-deck".to_owned(), vec![9]),
                ("added".to_owned(), vec![1]),
            ],
            keep: vec![KavitaKeep::Home, KavitaKeep::Shelf("on-deck".to_owned())],
            ..with_series(&[(1, "Wings"), (2, "Tide")])
        };
        assert!(publish(&db, "server-1", second));
        let shown = visible_kavita_mirror(&db, "server-1").expect("mirror");
        assert_eq!(
            shown
                .home
                .iter()
                .map(|row| row.stream_id)
                .collect::<Vec<_>>(),
            [1]
        );
        assert_eq!(
            shown.shelves.get("on-deck").map(Vec::as_slice),
            Some(&[1, 2][..])
        );
        assert_eq!(
            shown.shelves.get("added").map(Vec::as_slice),
            Some(&[1][..])
        );
        assert_eq!(shown.wants, [2]);
    }

    #[test]
    fn a_failed_push_survives_and_blocks_the_pull() {
        let db = test_db();
        enqueue_kavita_progress(&db, outbox(10, "page:2")).expect("enqueue");
        apply_kavita_pull(&db, "server-1", 9, "page:1", 11).expect("pull");
        let pending = kavita_pending(&db, "server-1", 9)
            .expect("row")
            .expect("pending");
        assert_eq!(pending.locator, "page:2");
        acknowledge_kavita_progress(&db, &pending).expect("ack");
        assert!(kavita_pending(&db, "server-1", 9).expect("gone").is_none());
    }

    #[test]
    fn an_acknowledgement_of_an_older_row_leaves_the_newer_one() {
        let db = test_db();
        enqueue_kavita_progress(&db, outbox(10, "page:2")).expect("first");
        let sent = kavita_pending(&db, "server-1", 9)
            .expect("row")
            .expect("pending");
        enqueue_kavita_progress(&db, outbox(12, "page:3")).expect("later page");
        acknowledge_kavita_progress(&db, &sent).expect("ack");
        let left = kavita_pending(&db, "server-1", 9)
            .expect("row")
            .expect("still pending");
        assert_eq!(left.locator, "page:3");
        enqueue_kavita_mark(&db, mark(10, true)).expect("mark");
        let sent = kavita_marks(&db, "server-1").expect("marks").remove(0);
        enqueue_kavita_mark(&db, mark(12, false)).expect("unmark");
        acknowledge_kavita_mark(&db, &sent).expect("ack mark");
        let marks = kavita_marks(&db, "server-1").expect("marks");
        assert_eq!(marks.len(), 1);
        assert!(!marks[0].read);
        acknowledge_kavita_mark(&db, &marks[0]).expect("ack newer");
        assert!(kavita_marks(&db, "server-1").expect("none").is_empty());
    }

    #[test]
    fn a_download_is_claimed_once_and_a_running_one_survives_an_enqueue() {
        let db = test_db();
        enqueue_kavita_download(&db, "ch-9", "server-1", "http://127.0.0.1:9/ch-9").expect("queue");
        assert!(claim_kavita_download(&db, "ch-9").expect("claim"));
        assert!(!claim_kavita_download(&db, "ch-9").expect("second claim"));
        enqueue_kavita_download(&db, "ch-9", "server-1", "http://127.0.0.1:9/ch-9").expect("again");
        assert_eq!(
            kavita_download_status(&db, "ch-9")
                .expect("status")
                .as_deref(),
            Some("running")
        );
        assert!(settle_kavita_download(&db, "ch-9", "done").expect("settle"));
        assert!(!settle_kavita_download(&db, "ch-9", "queued").expect("not running"));
        enqueue_kavita_download(&db, "ch-9", "server-1", "http://127.0.0.1:9/ch-9").expect("redo");
        assert_eq!(
            kavita_download_status(&db, "ch-9")
                .expect("status")
                .as_deref(),
            Some("queued")
        );
        db.work(|conn| {
            let items = catalog::items(conn).expect("items");
            assert!(items.is_empty());
            Ok(())
        })
        .expect("work");
    }

    #[test]
    fn resetting_or_forgetting_a_server_drops_its_mirror_in_one_step() {
        let db = test_db();
        assert!(publish(&db, "server-1", with_series(&[(1, "Wings")])));
        enqueue_kavita_progress(&db, outbox(10, "page:2")).expect("enqueue");
        reset_kavita_server(&db, stored("server-1", "http://moved.test")).expect("reset");
        assert!(series_names(&db, "server-1").is_empty());
        assert!(kavita_pending(&db, "server-1", 9).expect("row").is_none());
        let rows = servers(&db).expect("servers");
        assert!(rows
            .iter()
            .any(|row| row.id == "server-1" && row.url == "http://moved.test"));
        assert!(publish(&db, "server-2", with_series(&[(2, "Tide")])));
        forget_kavita_server(&db, "server-2").expect("forget");
        assert!(series_names(&db, "server-2").is_empty());
        assert!(servers(&db)
            .expect("servers")
            .iter()
            .all(|row| row.id != "server-2"));
        assert!(forget_kavita_server(&db, "server-2").is_err());
    }
}
