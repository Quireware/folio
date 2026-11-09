//! Reading statistics from stored sessions and catalog types.
//!
//! The clock is an argument. This module does not open a socket.

use std::collections::BTreeMap;

use crate::ReadingSession;

/// Milliseconds in one UTC day.
pub const DAY_MS: i64 = 86_400_000;

/// Counts the statistics screen shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadingStats {
    /// Consecutive days ending on `now_ms` with pages read.
    pub streak_days: u32,
    /// Pages in sessions that ended in the last 30 days.
    pub pages_last_30_days: i64,
    /// File types and how many library rows have each one, in type order.
    pub formats: Vec<(String, i64)>,
}

/// Summarise `sessions` and `file_types` at `now_ms`.
pub fn summarise(sessions: &[ReadingSession], file_types: &[String], now_ms: i64) -> ReadingStats {
    let mut pages_by_day: BTreeMap<i64, i64> = BTreeMap::new();
    let mut pages_last_30_days = 0i64;
    let window_start = now_ms.saturating_sub(DAY_MS.saturating_mul(30));
    for session in sessions {
        let Some(ended) = session.ended_at else {
            continue;
        };
        if session.pages_read <= 0 {
            continue;
        }
        if (window_start..=now_ms).contains(&ended) {
            pages_last_30_days = pages_last_30_days.saturating_add(session.pages_read);
        }
        let day = ended.div_euclid(DAY_MS);
        let pages = pages_by_day.entry(day).or_insert(0);
        *pages = pages.saturating_add(session.pages_read);
    }
    let mut streak_days = 0u32;
    let mut day = now_ms.div_euclid(DAY_MS);
    loop {
        match pages_by_day.get(&day) {
            Some(pages) if *pages > 0 => {
                streak_days = streak_days.saturating_add(1);
                day = day.saturating_sub(1);
            }
            _ => break,
        }
    }
    let mut counted: BTreeMap<&str, i64> = BTreeMap::new();
    for file_type in file_types {
        let count = counted.entry(file_type.as_str()).or_insert(0);
        *count = count.saturating_add(1);
    }
    ReadingStats {
        streak_days,
        pages_last_30_days,
        formats: counted
            .into_iter()
            .map(|(file_type, count)| (file_type.to_owned(), count))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::{summarise, DAY_MS};
    use crate::{
        is_complete, land_before_stop, Bookmark, CatalogItem, Database, PendingReading,
        ProgressDebounce, ReadingSession,
    };

    #[test]
    fn stats_cover_progress_sessions_bookmarks_and_stop() {
        let now = DAY_MS.saturating_mul(102) + 1_000;
        let sessions = [
            finished("s0", DAY_MS.saturating_mul(60), 9),
            finished("s1", DAY_MS.saturating_mul(100), 2),
            finished("s2", DAY_MS.saturating_mul(101), 3),
            finished("s3", DAY_MS.saturating_mul(102), 4),
        ];
        let stats = summarise(
            &sessions,
            &["cbz".to_owned(), "epub".to_owned(), "epub".to_owned()],
            now,
        );
        assert_eq!(stats.streak_days, 3);
        assert_eq!(stats.pages_last_30_days, 9);
        assert_eq!(
            stats.formats,
            vec![("cbz".to_owned(), 1), ("epub".to_owned(), 2)]
        );

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("folio.sqlite");
        {
            let db = Database::open(&path).expect("open");
            db.start_session("s1", "book-1", DAY_MS).expect("start");
            db.finish_session("s1", DAY_MS + 10, 4).expect("finish");
            db.insert_bookmark(Bookmark {
                id: "mark-1".to_owned(),
                book_id: "book-1".to_owned(),
                locator: "epubcfi(/6/4)".to_owned(),
                name: "Cave".to_owned(),
            })
            .expect("bookmark");
            db.rename_bookmark("mark-1", "Return").expect("rename");
            db.upsert_item(sample_item("book-1", "epub")).expect("item");
            let mut debounce = ProgressDebounce::new(5_000);
            debounce.note(pending(false), 0);
            assert!(debounce.poll(4_999).is_none());
            land_before_stop(&db, &mut debounce).expect("stop");
        }
        let db = Database::open(&path).expect("reopen");
        let reading = db.reading("book-1").expect("reading").expect("row");
        assert_eq!(reading.locator, "epubcfi(/6/4!/4/2:0)");
        assert_eq!(reading.page, Some(i64::from(3)));
        assert_eq!(reading.chapter.as_deref(), Some("c2"));
        assert!(!reading.complete);
        let sessions = db.sessions().expect("sessions");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].pages_read, 4);
        assert_eq!(sessions[0].ended_at, Some(DAY_MS + 10));
        let marks = db.bookmarks("book-1").expect("marks");
        assert_eq!(marks[0].name, "Return");
        assert_eq!(marks[0].locator, "epubcfi(/6/4)");
        db.delete_bookmark("mark-1").expect("delete");
        assert!(db.bookmarks("book-1").expect("empty").is_empty());
        let types: Vec<String> = db
            .catalog()
            .expect("catalog")
            .into_iter()
            .map(|item| item.file_type)
            .collect();
        let reopened = summarise(&db.sessions().expect("sessions"), &types, DAY_MS + 10);
        assert_eq!(reopened.formats, vec![("epub".to_owned(), 1)]);
        assert!(!is_complete(3, 4, false));
        assert!(is_complete(3, 4, true));
    }

    fn finished(id: &str, ended_at: i64, pages: i64) -> ReadingSession {
        ReadingSession {
            id: id.to_owned(),
            book_id: "book-1".to_owned(),
            started_at: ended_at,
            ended_at: Some(ended_at),
            pages_read: pages,
        }
    }

    fn pending(mark_last_page_complete: bool) -> PendingReading {
        PendingReading {
            book_id: "book-1".to_owned(),
            locator: "epubcfi(/6/4!/4/2:0)".to_owned(),
            page: 3,
            chapter: "c2".to_owned(),
            page_count: 4,
            mark_last_page_complete,
        }
    }

    fn sample_item(id: &str, file_type: &str) -> CatalogItem {
        CatalogItem {
            id: id.to_owned(),
            source: id.to_owned(),
            label: id.to_owned(),
            file_type: file_type.to_owned(),
            size: None,
            modified_ms: None,
            series_guess: String::new(),
            series: None,
            title: id.to_owned(),
            page_count: None,
            image_child_count: None,
            fingerprint: String::new(),
            enriched_fingerprint: None,
        }
    }
}
