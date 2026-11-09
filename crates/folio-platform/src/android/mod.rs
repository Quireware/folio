//! Document-cursor listing, foreground-service gate, and external SD roots.
//!
//! Pass 1 does not open a stream. A tree grant is not an external SD volume.

mod keys;
mod monet;
mod saf;
mod service;
mod widget;

pub use keys::{binding, KeyLayout, ReaderCommand};
pub use monet::{resolve_theme, ResolvedTheme, SystemPalette};
pub use saf::{AndroidError, ExternalSdRoot};
pub use service::{BackgroundJob, ServiceGate};
pub use widget::{shortcuts, WidgetSnapshot};

use crate::list::{CursorListing, ListedPath};

/// One row from an Android document cursor.
///
/// A null `COLUMN_SIZE` or `COLUMN_LAST_MODIFIED` is [`None`], not zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorDocument {
    /// Path relative to the granted tree, using `/`.
    pub relative_path: String,
    /// `COLUMN_SIZE` when the provider set it.
    pub size: Option<u64>,
    /// `COLUMN_LAST_MODIFIED` in Unix milliseconds when the provider set it.
    pub modified_ms: Option<i64>,
    /// MIME type from the cursor, when present.
    pub mime: Option<String>,
    /// Whether the document is a directory.
    pub is_directory: bool,
}

/// Documents already read from a cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentCursor {
    /// Rows the cursor yielded. This adapter does not call `openInputStream`.
    pub documents: Vec<CursorDocument>,
    /// Android `EXTRA_LOADING`. A loading cursor is not a finished listing.
    pub extra_loading: bool,
}

/// Convert a cursor into listing metadata.
///
/// Null sizes and modification times stay unknown. They are not rewritten as zero.
pub fn list_document_cursor(cursor: &DocumentCursor) -> CursorListing {
    let entries = cursor
        .documents
        .iter()
        .map(|document| ListedPath {
            // to_owned: the listing outlives the cursor borrow
            relative: document.relative_path.to_owned(),
            size: document.size,
            modified_ms: document.modified_ms,
            // clone: the listing owns its MIME string after the cursor is dropped
            mime: document.mime.clone(),
            is_dir: document.is_directory,
        })
        .collect();
    CursorListing {
        entries,
        complete: !cursor.extra_loading,
    }
}

#[cfg(test)]
mod tests {
    use super::{list_document_cursor, CursorDocument, DocumentCursor};

    #[test]
    fn null_size_stays_unknown_and_loading_is_incomplete() {
        let cursor = DocumentCursor {
            extra_loading: true,
            documents: vec![CursorDocument {
                relative_path: "a.pdf".to_owned(),
                size: None,
                modified_ms: None,
                mime: Some("application/pdf".to_owned()),
                is_directory: false,
            }],
        };
        let listed = list_document_cursor(&cursor);
        assert!(!listed.complete);
        assert_eq!(listed.entries[0].size, None);
        assert_eq!(listed.entries[0].modified_ms, None);
        assert_ne!(listed.entries[0].size, Some(0));
    }
}
