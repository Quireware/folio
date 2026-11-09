//! Chrome destinations. Download is not a route. Item detail stays Read.

/// A chrome destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// The library.
    Library,
    /// Reading statistics.
    Statistics,
    /// Settings.
    Settings,
    /// Opened books. Added with collections; first ship did not have this tab.
    History,
    /// Saved servers. Added with OPDS; first ship did not have this tab.
    Servers,
}

/// Chrome routes, in tab order.
pub fn routes() -> &'static [Route] {
    &[
        Route::Library,
        Route::Statistics,
        Route::Settings,
        Route::History,
        Route::Servers,
    ]
}

/// Stable route name.
pub fn route_name(route: Route) -> &'static str {
    match route {
        Route::Library => "library",
        Route::Statistics => "statistics",
        Route::Settings => "settings",
        Route::History => "history",
        Route::Servers => "servers",
    }
}

/// Item detail offers Read and nothing else.
pub fn item_actions() -> &'static [&'static str] {
    &["read"]
}

/// Minimum touch target in the chrome, in dp.
pub const MIN_TOUCH_DP: u32 = 48;

/// Whether a control of `dp` meets [`MIN_TOUCH_DP`].
pub fn touch_target_ok(dp: u32) -> bool {
    dp >= MIN_TOUCH_DP
}

/// Wide from 600dp. Narrow below that.
pub fn wide_layout(width_dp: u32) -> bool {
    width_dp >= 600
}

/// TalkBack labels for the chrome tabs.
pub fn chrome_labels() -> &'static [(&'static str, &'static str)] {
    &[
        ("library", "Library"),
        ("statistics", "Statistics"),
        ("settings", "Settings"),
        ("history", "History"),
        ("servers", "Servers"),
    ]
}

/// One home shelf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shelf {
    /// Shelf name.
    pub name: &'static str,
    /// Book ids, newest first where that applies.
    pub book_ids: Vec<String>,
}

/// A progress row the shelves read. This is not a History tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShelfProgress {
    /// Book id.
    pub book_id: String,
    /// Unix milliseconds.
    pub updated_at: i64,
    /// Finished books leave Continue Reading.
    pub complete: bool,
    /// Zero-based page, or chapter for EPUB and MOBI, when the reader saved one.
    pub page: Option<i64>,
}

/// Reading position shown on a Continue card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressLine {
    /// "Chapter 3 of 24" or "Page 233 of 612".
    pub label: String,
    /// Whole percent read, 0 to 100.
    pub percent: u8,
}

/// Position for a zero-based `page` of `page_count`. EPUB and MOBI count chapters; other formats count pages.
/// Returns `None` until the scan has stored a page count.
pub fn progress_line(
    file_type: &str,
    page: Option<i64>,
    page_count: Option<i64>,
) -> Option<ProgressLine> {
    let total = page_count.filter(|count| *count > 0)?;
    let shown = page?.max(0).saturating_add(1).min(total);
    let unit = if matches!(file_type, "epub" | "mobi") {
        "Chapter"
    } else {
        "Page"
    };
    let percent = u8::try_from(shown.saturating_mul(100) / total).unwrap_or(100);
    Some(ProgressLine {
        label: format!("{unit} {shown} of {total}"),
        percent,
    })
}

/// Continue Reading, Recently Read, then the catalog.
pub fn home_shelves(progress: &[ShelfProgress], catalog: &[String]) -> Vec<Shelf> {
    let mut ordered = progress.to_vec();
    ordered.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.book_id.cmp(&right.book_id))
    });
    let continue_reading = ordered
        .iter()
        .filter(|row| !row.complete)
        .map(|row| row.book_id.clone())
        .collect();
    let recently_read = ordered.iter().map(|row| row.book_id.clone()).collect();
    vec![
        Shelf {
            name: "continue-reading",
            book_ids: continue_reading,
        },
        Shelf {
            name: "recently-read",
            book_ids: recently_read,
        },
        Shelf {
            name: "catalog",
            // clone: the shelf owns its ids
            book_ids: catalog.to_vec(),
        },
    ]
}

/// A library row search can see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRow {
    /// Book id.
    pub id: String,
    /// Filename label from pass 1.
    pub label: String,
    /// Title from pass 2. Equal to the label until enrichment.
    pub title: String,
    /// Pass 2 finished.
    pub enriched: bool,
}

/// Local search. Unenriched rows match the filename. Enriched rows match the title.
pub fn search_local<'a>(rows: &'a [SearchRow], query: &str) -> Vec<&'a str> {
    let folded = query.to_ascii_lowercase();
    if folded.is_empty() {
        return Vec::new();
    }
    rows.iter()
        .filter(|row| {
            let field = if row.enriched { &row.title } else { &row.label };
            field.to_ascii_lowercase().contains(&folded)
        })
        .map(|row| row.id.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        chrome_labels, home_shelves, item_actions, progress_line, route_name, routes, search_local,
        touch_target_ok, wide_layout, ProgressLine, Route, SearchRow, ShelfProgress,
    };

    #[test]
    fn shell_routes_shelves_and_search() {
        let names: Vec<&str> = routes().iter().copied().map(route_name).collect();
        assert_eq!(
            names,
            ["library", "statistics", "settings", "history", "servers"]
        );
        // History is registered because phase 11 added it. First ship did not have this tab.
        assert!(names.contains(&"history"));
        // Servers is registered because phase 15 added it. Download stays off this list and off item detail.
        assert!(names.contains(&"servers"));
        assert!(!names.contains(&"download"));
        assert_eq!(item_actions(), ["read"]);
        assert!(!item_actions().contains(&"download"));
        assert!(!touch_target_ok(47));
        assert!(touch_target_ok(48));
        assert!(!wide_layout(599));
        assert!(wide_layout(600));
        assert_eq!(chrome_labels().len(), 5);

        let progress = [
            ShelfProgress {
                book_id: "old".to_owned(),
                page: None,
                updated_at: 1,
                complete: false,
            },
            ShelfProgress {
                book_id: "done".to_owned(),
                page: None,
                updated_at: 3,
                complete: true,
            },
            ShelfProgress {
                book_id: "new".to_owned(),
                page: None,
                updated_at: 2,
                complete: false,
            },
        ];
        let shelves = home_shelves(&progress, &["old".to_owned(), "new".to_owned()]);
        assert_eq!(shelves[0].name, "continue-reading");
        assert_eq!(shelves[0].book_ids, ["new", "old"]);
        assert_eq!(shelves[1].name, "recently-read");
        assert_eq!(shelves[1].book_ids, ["done", "new", "old"]);
        assert_eq!(shelves[2].book_ids, ["old", "new"]);

        let rows = [
            SearchRow {
                id: "file".to_owned(),
                label: "Dune.epub".to_owned(),
                title: "Dune.epub".to_owned(),
                enriched: false,
            },
            SearchRow {
                id: "book".to_owned(),
                label: "book.cbz".to_owned(),
                title: "Arrakis".to_owned(),
                enriched: true,
            },
        ];
        assert_eq!(search_local(&rows, "dune"), ["file"]);
        assert_eq!(search_local(&rows, "arrakis"), ["book"]);
        assert!(search_local(&rows, "book.cbz").is_empty());
        let _ = Route::Library;
    }

    #[test]
    fn progress_line_counts_chapters_for_text_and_pages_otherwise() {
        assert_eq!(
            progress_line("epub", Some(2), Some(24)),
            Some(ProgressLine {
                label: "Chapter 3 of 24".to_owned(),
                percent: 12,
            })
        );
        assert_eq!(
            progress_line("pdf", Some(232), Some(612)),
            Some(ProgressLine {
                label: "Page 233 of 612".to_owned(),
                percent: 38,
            })
        );
        assert_eq!(
            progress_line("cbz", Some(9), Some(10)).map(|line| line.percent),
            Some(100)
        );
        assert_eq!(
            progress_line("pdf", Some(40), Some(10)).map(|line| line.label),
            Some("Page 10 of 10".to_owned())
        );
        assert_eq!(progress_line("epub", Some(2), None), None);
        assert_eq!(progress_line("epub", None, Some(24)), None);
        assert_eq!(progress_line("pdf", Some(0), Some(0)), None);
    }
}
