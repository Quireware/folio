//! Progress delivery. Only a server with a progress-write fixture accepts a page.

use std::collections::BTreeMap;

use crate::browse::ServerError;
use crate::{komga, librarian, ServerKind};

/// Pages stored by a progress-write fixture. This is not a network client.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProgressFixture {
    pages: BTreeMap<String, u32>,
}

impl ProgressFixture {
    /// Stored page for `book_id`.
    pub fn page(&self, book_id: &str) -> Option<u32> {
        self.pages.get(book_id).copied()
    }

    pub(crate) fn insert_page(&mut self, book_id: &str, page: u32) {
        // to_owned: the fixture map owns the book id
        self.pages.insert(book_id.to_owned(), page);
    }
}

/// Whether `kind` can store a progress write.
pub fn progress_write_supported(kind: ServerKind) -> bool {
    match kind {
        ServerKind::Komga => komga::progress_write_supported(),
        ServerKind::Librarian => librarian::progress_write_supported(),
        ServerKind::Kavita | ServerKind::OpdsV1 | ServerKind::OpdsV2 => false,
    }
}

/// Apply `locator` (`page:<n>`) when `kind` has a progress write.
///
/// Plain OPDS and librarian return [`ServerError::Unsupported`]. They are not reported as success.
///
/// # Errors
///
/// Returns [`ServerError::Unsupported`] when the server cannot store progress, and
/// [`ServerError::Feed`] when the locator is not `page:<n>` or the book id is empty.
pub fn deliver_progress(
    kind: ServerKind,
    fixture: &mut ProgressFixture,
    book_id: &str,
    locator: &str,
) -> Result<(), ServerError> {
    if book_id.is_empty() || locator.is_empty() {
        return Err(ServerError::Feed);
    }
    match kind {
        ServerKind::Komga => komga::store_progress(fixture, book_id, locator),
        ServerKind::Librarian | ServerKind::Kavita | ServerKind::OpdsV1 | ServerKind::OpdsV2 => {
            Err(ServerError::Unsupported)
        }
    }
}
