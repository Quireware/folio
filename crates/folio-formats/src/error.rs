//! Failures a parser can report without panicking.

use std::io;

use thiserror::Error;

/// A publication could not be opened.
#[derive(Debug, Error)]
pub enum FormatError {
    /// `META-INF/encryption.xml` is present. Folio does not decrypt books.
    #[error("the publication is encrypted")]
    Drm,
    /// An entry is absolute or contains `..`.
    #[error("an archive entry escapes its root")]
    PathEscape,
    /// The container is missing a required part or is not well-formed.
    #[error("the publication is not a valid {kind} file")]
    Invalid {
        /// Which parser rejected the file.
        kind: &'static str,
    },
    /// The Pdfium shared library could not be loaded. Folio does not vendor it.
    #[error("pdfium is not available")]
    PdfUnavailable,
    /// Reading the source failed.
    #[error("the publication could not be read")]
    Io(#[source] io::Error),
}

impl From<io::Error> for FormatError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
