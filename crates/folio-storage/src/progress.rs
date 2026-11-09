use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};

use crate::StorageError;

pub(crate) fn set_progress(
    conn: &Connection,
    book_id: &str,
    locator: &str,
) -> Result<(), StorageError> {
    let updated_at = now_millis()?;
    conn.execute(
        "INSERT INTO progress (book_id, locator, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(book_id) DO UPDATE SET locator = excluded.locator, updated_at = excluded.updated_at",
        params![book_id, locator, updated_at],
    )?;
    Ok(())
}

pub(crate) fn progress(conn: &Connection, book_id: &str) -> Result<Option<String>, StorageError> {
    optional_string(
        conn,
        "SELECT locator FROM progress WHERE book_id = ?1",
        [book_id],
    )
}

pub(crate) fn latest_locator(conn: &Connection) -> Result<Option<String>, StorageError> {
    match conn.query_row(
        "SELECT locator FROM progress ORDER BY updated_at DESC, book_id ASC LIMIT 1",
        [],
        |row| row.get(0),
    ) {
        Ok(locator) => Ok(Some(locator)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn optional_string(
    conn: &Connection,
    sql: &str,
    params: [&str; 1],
) -> Result<Option<String>, StorageError> {
    match conn.query_row(sql, params, |row| row.get(0)) {
        Ok(locator) => Ok(Some(locator)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Fields written by a debounced reader save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadingProgress {
    /// Locator string. Existing callers still store a CFI here.
    pub locator: String,
    /// Zero-based page, when the reader sent one.
    pub page: Option<i64>,
    /// Chapter id, when the reader sent one.
    pub chapter: Option<String>,
    /// True when the reader marked the book complete.
    pub complete: bool,
    /// Unix milliseconds of the write.
    pub updated_at: i64,
}

/// One unsaved reader position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingReading {
    /// Book id.
    pub book_id: String,
    /// CFI or page locator.
    pub locator: String,
    /// Zero-based page.
    pub page: u32,
    /// Chapter id.
    pub chapter: String,
    /// Pages in the book.
    pub page_count: u32,
    /// Setting: the last page counts as finished.
    pub mark_last_page_complete: bool,
}

/// Holds the newest position until the delay passes, or until the activity stops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressDebounce {
    delay_ms: i64,
    pending: Option<PendingReading>,
    noted_at: i64,
}

impl ProgressDebounce {
    /// `delay_ms` is how long a note waits before [`Self::poll`] returns it.
    pub fn new(delay_ms: i64) -> Self {
        Self {
            delay_ms,
            pending: None,
            noted_at: 0,
        }
    }

    /// Replace any queued position. The clock is `now_ms`.
    pub fn note(&mut self, pending: PendingReading, now_ms: i64) {
        self.pending = Some(pending);
        self.noted_at = now_ms;
    }

    /// The queued position once the delay has elapsed.
    pub fn poll(&mut self, now_ms: i64) -> Option<PendingReading> {
        if now_ms.saturating_sub(self.noted_at) < self.delay_ms {
            return None;
        }
        self.pending.take()
    }

    /// The queued position and the clock value from [`Self::note`], ignoring the delay.
    pub fn flush(&mut self) -> Option<(PendingReading, i64)> {
        self.pending.take().map(|pending| (pending, self.noted_at))
    }
}

/// Last-page completion. Off unless `mark_last_page_complete` is set.
pub fn is_complete(page: u32, page_count: u32, mark_last_page_complete: bool) -> bool {
    mark_last_page_complete && page_count > 0 && page.saturating_add(1) == page_count
}

/// Write a queued position before the caller returns from stop.
///
/// # Errors
///
/// Returns an error when the writer cannot commit.
pub fn land_before_stop(
    db: &crate::Database,
    debounce: &mut ProgressDebounce,
) -> Result<(), StorageError> {
    let Some((pending, noted_at)) = debounce.flush() else {
        return Ok(());
    };
    let complete = is_complete(
        pending.page,
        pending.page_count,
        pending.mark_last_page_complete,
    );
    db.set_reading(
        &pending.book_id,
        &pending.locator,
        pending.page,
        &pending.chapter,
        complete,
        noted_at,
    )
}

pub(crate) fn set_reading(
    conn: &Connection,
    book_id: &str,
    locator: &str,
    page: i64,
    chapter: &str,
    complete: i64,
    updated_at: i64,
) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO progress (book_id, locator, updated_at, page, chapter, complete)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(book_id) DO UPDATE SET
            locator = excluded.locator,
            updated_at = excluded.updated_at,
            page = excluded.page,
            chapter = excluded.chapter,
            complete = excluded.complete",
        params![book_id, locator, updated_at, page, chapter, complete],
    )?;
    Ok(())
}

pub(crate) fn reading(
    conn: &Connection,
    book_id: &str,
) -> Result<Option<ReadingProgress>, StorageError> {
    match conn.query_row(
        "SELECT locator, page, chapter, COALESCE(complete, 0), updated_at
         FROM progress WHERE book_id = ?1",
        [book_id],
        |row| {
            let complete: i64 = row.get(3)?;
            Ok(ReadingProgress {
                locator: row.get(0)?,
                page: row.get(1)?,
                chapter: row.get(2)?,
                complete: complete != 0,
                updated_at: row.get(4)?,
            })
        },
    ) {
        Ok(progress) => Ok(Some(progress)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn now_millis() -> Result<i64, StorageError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| StorageError::Clock(error.to_string()))?;
    i64::try_from(elapsed.as_millis()).map_err(|error| StorageError::Clock(error.to_string()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn progress_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("folio.sqlite");
        {
            let db = crate::Database::open(&path).unwrap();
            db.set_progress("book-1", "epubcfi(/6/2)").unwrap();
        }
        let db = crate::Database::open(&path).unwrap();
        assert_eq!(
            db.progress("book-1").unwrap().as_deref(),
            Some("epubcfi(/6/2)")
        );
    }
}
