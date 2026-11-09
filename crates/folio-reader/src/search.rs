//! In-book search. The caller passes chapter text it already opened.
//!
//! Nothing here lists a library directory or opens an archive.

/// One match inside a chapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    /// Chapter id.
    pub chapter: String,
    /// Text around the match.
    pub snippet: String,
}

/// Find `query` in chapters that are already in memory.
///
/// An empty query matches nothing. The match is ASCII case-insensitive.
pub fn search_chapters(chapters: &[(String, String)], query: &str) -> Vec<SearchHit> {
    if query.is_empty() {
        return Vec::new();
    }
    let needle = query.to_ascii_lowercase();
    chapters
        .iter()
        .filter_map(|(chapter, body)| snippet(chapter, body, &needle))
        .collect()
}

fn snippet(chapter: &str, body: &str, needle: &str) -> Option<SearchHit> {
    let folded = body.to_ascii_lowercase();
    let index = folded.find(needle)?;
    let start = floor_char(body, index.saturating_sub(12));
    let end = ceil_char(body, index.saturating_add(needle.len()).saturating_add(12));
    Some(SearchHit {
        // to_owned: the hit outlives the chapter borrow
        chapter: chapter.to_owned(),
        snippet: body[start..end].to_owned(),
    })
}

fn floor_char(text: &str, mut index: usize) -> usize {
    if index > text.len() {
        index = text.len();
    }
    while index > 0 && !text.is_char_boundary(index) {
        index = index.saturating_sub(1);
    }
    index
}

fn ceil_char(text: &str, mut index: usize) -> usize {
    if index > text.len() {
        return text.len();
    }
    while index < text.len() && !text.is_char_boundary(index) {
        index = index.saturating_add(1);
    }
    index
}

#[cfg(test)]
mod tests {
    use super::search_chapters;
    use folio_storage::Database;

    #[test]
    fn search_snippet_comes_from_open_chapters_not_a_scan() {
        let chapters = vec![("c1".to_owned(), "Hello folio world".to_owned())];
        let hits = search_chapters(&chapters, "Folio");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.to_ascii_lowercase().contains("folio"));
        assert!(search_chapters(&chapters, "").is_empty());
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("folio.sqlite");
        {
            let db = Database::open(&path).expect("open");
            db.replace_search("book-1", chapters).expect("index");
        }
        let db = Database::open(&path).expect("reopen");
        let stored = db.search_chapters("book-1").expect("chapters");
        let again = search_chapters(&stored, "world");
        assert!(again[0].snippet.contains("world"));
    }
}
