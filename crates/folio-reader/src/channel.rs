//! Reports the content frame may post. It cannot call the chrome bridge.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A report from the content document. Privileged chrome calls are not variants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContentReport {
    /// The reader moved to a page.
    Page {
        /// Book id.
        book_id: String,
        /// Zero-based page.
        page: u32,
    },
    /// The reader moved to a CFI.
    Cfi {
        /// Book id.
        book_id: String,
        /// EPUB CFI.
        cfi: String,
    },
}

/// Why a content-frame payload was refused.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ChannelError {
    /// The payload is not a [`ContentReport`].
    #[error("the content frame sent a payload the channel does not accept")]
    Rejected,
}

/// Parse a content-frame report.
///
/// Strings that name the chrome bridge, `eval`, or the Android IPC object are
/// rejected even when the rest of the payload would otherwise parse.
///
/// # Errors
///
/// Returns [`ChannelError::Rejected`] when `body` is not a page or CFI report.
pub fn accept_content_report(body: &str) -> Result<ContentReport, ChannelError> {
    if privileged(body) {
        return Err(ChannelError::Rejected);
    }
    serde_json::from_str(body).map_err(|_| ChannelError::Rejected)
}

/// Document the content frame loads. It does not include the chrome bridge.
pub fn content_document(chapter_html: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"></head><body>{chapter_html}</body></html>"
    )
}

fn privileged(body: &str) -> bool {
    let folded = body.to_ascii_lowercase();
    folded.contains("addjavascriptinterface")
        || folded.contains("dioxus")
        || folded.contains("document.eval")
        || folded.contains(".eval(")
        || folded.contains("dataset.folio")
}

#[cfg(test)]
mod tests {
    use super::{accept_content_report, content_document, ContentReport};

    #[test]
    fn content_frame_rejects_privileged_calls() {
        let chapter = "<p>Hello</p><img src=\"pic.png\">";
        let document = content_document(chapter);
        let folded = document.to_ascii_lowercase();
        assert!(!folded.contains("script"));
        assert!(!folded.contains("dioxus"));
        assert!(!folded.contains("addjavascriptinterface"));
        assert!(!folded.contains("eval"));
        assert!(!folded.contains("dataset.folio"));
        assert!(document.contains("Hello"));

        let page = accept_content_report(r#"{"kind":"page","book_id":"book-1","page":2}"#)
            .expect("page report");
        assert_eq!(
            page,
            ContentReport::Page {
                book_id: "book-1".to_owned(),
                page: 2,
            }
        );
        let cfi =
            accept_content_report(r#"{"kind":"cfi","book_id":"book-1","cfi":"epubcfi(/6/2)"}"#)
                .expect("cfi report");
        assert!(matches!(cfi, ContentReport::Cfi { .. }));
        assert!(accept_content_report(r#"{"kind":"eval","body":"document.eval"}"#).is_err());
        assert!(accept_content_report(
            r#"{"kind":"page","book_id":"addJavascriptInterface","page":0}"#
        )
        .is_err());
    }
}
