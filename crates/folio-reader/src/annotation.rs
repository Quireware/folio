//! Highlights anchored by a CFI.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Why a highlight could not be stored or read.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AnnotationError {
    /// An id, book id, or CFI was empty.
    #[error("highlight id, book id, and cfi must not be empty")]
    Empty,
    /// The stored body was not highlight JSON.
    #[error("highlight body was not json")]
    Body,
}

/// Highlight colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Colour {
    /// Yellow.
    Yellow,
    /// Green.
    Green,
    /// Blue.
    Blue,
    /// Pink.
    Pink,
}

/// One highlight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Highlight {
    /// Caller-chosen id.
    pub id: String,
    /// Book id.
    pub book_id: String,
    /// EPUB CFI.
    pub cfi: String,
    /// Colour.
    pub colour: Colour,
    /// Optional note.
    pub note: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct HighlightBody {
    colour: Colour,
    note: Option<String>,
}

impl Highlight {
    /// Build a highlight.
    ///
    /// # Errors
    ///
    /// Returns an error when `id`, `book_id`, or `cfi` is empty.
    pub fn new(
        id: impl Into<String>,
        book_id: impl Into<String>,
        cfi: impl Into<String>,
        colour: Colour,
        note: Option<String>,
    ) -> Result<Self, AnnotationError> {
        let highlight = Self {
            id: id.into(),
            book_id: book_id.into(),
            cfi: cfi.into(),
            colour,
            note,
        };
        if highlight.id.is_empty() || highlight.book_id.is_empty() || highlight.cfi.is_empty() {
            return Err(AnnotationError::Empty);
        }
        Ok(highlight)
    }

    /// JSON stored in the annotation body column.
    ///
    /// # Errors
    ///
    /// Returns an error when the body cannot be encoded.
    pub fn body_json(&self) -> Result<String, AnnotationError> {
        serde_json::to_string(&HighlightBody {
            colour: self.colour,
            // clone: the JSON body owns the note; the highlight keeps its own
            note: self.note.clone(),
        })
        .map_err(|_| AnnotationError::Body)
    }

    /// Rebuild a highlight from a stored row.
    ///
    /// # Errors
    ///
    /// Returns an error when `body` is not highlight JSON or an id is empty.
    pub fn from_row(
        id: &str,
        book_id: &str,
        cfi: &str,
        body: &str,
    ) -> Result<Self, AnnotationError> {
        let parsed: HighlightBody =
            serde_json::from_str(body).map_err(|_| AnnotationError::Body)?;
        Self::new(id, book_id, cfi, parsed.colour, parsed.note)
    }
}

/// Markdown export. One bullet per highlight.
pub fn markdown_export(highlights: &[Highlight]) -> String {
    let mut out = String::from("# Highlights\n");
    for highlight in highlights {
        out.push_str("- ");
        out.push_str(colour_name(highlight.colour));
        out.push(' ');
        out.push_str(&highlight.cfi);
        out.push('\n');
        if let Some(note) = &highlight.note {
            out.push_str(note);
            out.push('\n');
        }
    }
    out
}

/// JSON export of the highlights.
///
/// # Errors
///
/// Returns an error when the list cannot be encoded.
pub fn json_export(highlights: &[Highlight]) -> Result<String, AnnotationError> {
    serde_json::to_string(highlights).map_err(|_| AnnotationError::Body)
}

fn colour_name(colour: Colour) -> &'static str {
    match colour {
        Colour::Yellow => "yellow",
        Colour::Green => "green",
        Colour::Blue => "blue",
        Colour::Pink => "pink",
    }
}

#[cfg(test)]
mod tests {
    use super::{json_export, markdown_export, Colour, Highlight};
    use folio_storage::{Database, StoredAnnotation};

    #[test]
    fn annotation_reopen_export_and_delete() {
        let highlight = Highlight::new(
            "h1",
            "book-1",
            "epubcfi(/6/4!/4/2:1)",
            Colour::Yellow,
            Some("A note".to_owned()),
        )
        .expect("highlight");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("folio.sqlite");
        {
            let db = Database::open(&path).expect("open");
            db.upsert_annotation(StoredAnnotation {
                id: highlight.id.clone(),
                book_id: highlight.book_id.clone(),
                locator: highlight.cfi.clone(),
                body: highlight.body_json().expect("body"),
            })
            .expect("save");
        }
        let db = Database::open(&path).expect("reopen");
        let rows = db.annotations("book-1").expect("rows");
        let loaded = Highlight::from_row(
            &rows[0].id,
            &rows[0].book_id,
            &rows[0].locator,
            &rows[0].body,
        )
        .expect("row");
        assert_eq!(loaded, highlight);
        let markdown = markdown_export(std::slice::from_ref(&loaded));
        assert!(markdown.contains("yellow"));
        assert!(markdown.contains("A note"));
        let json = json_export(std::slice::from_ref(&loaded)).expect("json");
        assert!(json.contains("yellow"));
        assert!(!json.contains("password"));
        db.delete_annotation("h1").expect("delete");
        assert!(db.annotations("book-1").expect("gone").is_empty());
    }
}
