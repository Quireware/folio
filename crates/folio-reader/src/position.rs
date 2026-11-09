//! Positions the two readers share.

use serde::{Deserialize, Serialize};

/// A comic or PDF page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PagePosition {
    /// Book the page belongs to.
    pub book_id: String,
    /// Zero-based page index.
    pub page: u32,
}

/// An EPUB CFI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CfiPosition {
    /// Book the CFI belongs to.
    pub book_id: String,
    /// EPUB CFI, including the `epubcfi(` wrapper when the caller stored one.
    pub cfi: String,
}

#[cfg(test)]
mod tests {
    use super::{CfiPosition, PagePosition};
    use crate::ReadingMode;

    #[test]
    fn positions_and_mode_round_trip() {
        let page = PagePosition {
            book_id: "book-1".to_owned(),
            page: 3,
        };
        let cfi = CfiPosition {
            book_id: "book-1".to_owned(),
            cfi: "epubcfi(/6/2)".to_owned(),
        };
        let page_json = serde_json::to_string(&page).expect("page json");
        let cfi_json = serde_json::to_string(&cfi).expect("cfi json");
        let mode_json = serde_json::to_string(&ReadingMode::Webtoon).expect("mode json");
        assert_eq!(
            serde_json::from_str::<PagePosition>(&page_json).expect("page"),
            page
        );
        assert_eq!(
            serde_json::from_str::<CfiPosition>(&cfi_json).expect("cfi"),
            cfi
        );
        assert_eq!(
            serde_json::from_str::<ReadingMode>(&mode_json).expect("mode"),
            ReadingMode::Webtoon
        );
        assert_eq!(mode_json, "\"webtoon\"");
    }
}
