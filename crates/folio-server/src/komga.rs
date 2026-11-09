//! Komga library, series, and book fixtures.
//!
//! Progress on this path is a read. The progress-write fixture is [`store_progress`].

use crate::browse::{
    parse_progress, ParsedLibrary, RemoteBook, RemoteCatalog, RemoteLink, ServerError,
};
use crate::progress::ProgressFixture;
use serde::Deserialize;

/// Map Komga library, series, book, and progress fixtures into the shared browse types.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when a document is the wrong shape or a row has an empty id or URL.
pub fn parse_komga(
    libraries: &str,
    series: &str,
    books: &str,
    progress: &str,
) -> Result<ParsedLibrary, ServerError> {
    let libraries: Vec<NamedHref> =
        serde_json::from_str(libraries).map_err(|_| ServerError::Feed)?;
    let series: SeriesPage = serde_json::from_str(series).map_err(|_| ServerError::Feed)?;
    let books: BooksPage = serde_json::from_str(books).map_err(|_| ServerError::Feed)?;
    let mut children = Vec::new();
    for library in &libraries {
        children.push(RemoteLink::new(&library.name, &library.href)?);
    }
    for row in &series.content {
        children.push(RemoteLink::new(&row.name, &row.href)?);
    }
    let mut parsed_books = Vec::new();
    for book in &books.content {
        parsed_books.push(RemoteBook::new(&book.id, &book.name, &book.file)?);
    }
    Ok(ParsedLibrary::new(
        RemoteCatalog::new(children, parsed_books, None),
        parse_progress(progress)?,
    ))
}

/// Komga accepts a progress write. Other servers do not go through this function.
pub fn progress_write_supported() -> bool {
    true
}

pub(crate) fn store_progress(
    fixture: &mut ProgressFixture,
    book_id: &str,
    locator: &str,
) -> Result<(), ServerError> {
    let page = locator.strip_prefix("page:").ok_or(ServerError::Feed)?;
    let page: u32 = page.parse().map_err(|_| ServerError::Feed)?;
    fixture.insert_page(book_id, page);
    Ok(())
}

#[derive(Debug, Deserialize)]
struct NamedHref {
    name: String,
    href: String,
}

#[derive(Debug, Deserialize)]
struct SeriesPage {
    content: Vec<NamedHref>,
}

#[derive(Debug, Deserialize)]
struct BooksPage {
    content: Vec<BookRow>,
}

#[derive(Debug, Deserialize)]
struct BookRow {
    id: String,
    name: String,
    file: String,
}

#[cfg(test)]
mod tests {
    use super::parse_komga;
    use crate::browse::{enqueue_urls, export_auth};
    use crate::progress::{deliver_progress, progress_write_supported};
    use crate::{AuthConfig, ProgressFixture, ServerKind};

    #[test]
    fn komga_library_series_and_books_queue_a_download() {
        let parsed = parse_komga(
            include_str!("../tests/fixtures/komga/libraries.json"),
            include_str!("../tests/fixtures/komga/series.json"),
            include_str!("../tests/fixtures/komga/books.json"),
            include_str!("../tests/fixtures/komga/progress.json"),
        )
        .expect("komga");
        let catalog = parsed.catalog();
        assert_eq!(catalog.children()[0].title(), "Books");
        assert_eq!(catalog.children()[1].title(), "Dune");
        assert_eq!(catalog.books()[0].title(), "Dune");
        assert_eq!(catalog.books()[0].id(), "book");
        assert_eq!(parsed.progress().page(), 2);
        assert!(!parsed.progress().is_complete());

        let mut queued = Vec::new();
        enqueue_urls(catalog.books(), |url| queued.push(url.to_owned()));
        assert_eq!(queued, ["http://127.0.0.1:9/api/v1/books/book/file"]);

        let raw = "raw-api-key";
        let exported = export_auth(&AuthConfig::KomgaSession {
            secret_ref: "keyring:komga:local".into(),
        })
        .expect("export");
        assert!(exported.contains("keyring:komga:local"));
        assert!(!exported.contains(raw));
        assert!(!exported.contains("password"));
    }

    #[test]
    fn komga_progress_write_stores_the_page() {
        // Phase 17 turns on Komga's progress write. Kavita and librarian stay unsupported.
        assert!(progress_write_supported(ServerKind::Komga));
        assert!(!progress_write_supported(ServerKind::Kavita));
        assert!(!progress_write_supported(ServerKind::Librarian));
        assert!(!progress_write_supported(ServerKind::OpdsV2));
        let mut fixture = ProgressFixture::default();
        deliver_progress(ServerKind::Komga, &mut fixture, "book", "page:4").expect("write");
        assert_eq!(fixture.page("book"), Some(4));
        assert!(deliver_progress(ServerKind::Librarian, &mut fixture, "book", "page:4").is_err());
        assert!(deliver_progress(ServerKind::OpdsV1, &mut fixture, "book", "page:4").is_err());
    }
}
