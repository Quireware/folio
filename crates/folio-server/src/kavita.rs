//! Kavita library fixtures. Progress is read-only; Komga owns the progress write.

use crate::browse::{
    ParsedLibrary, RemoteBook, RemoteCatalog, RemoteLink, RemoteProgress, ServerError,
};
use serde::Deserialize;

/// Map a Kavita library document into the same browse types as Komga.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the document is the wrong shape or a row is empty.
pub fn parse_kavita(document: &str) -> Result<ParsedLibrary, ServerError> {
    let parsed: KavitaJson = serde_json::from_str(document).map_err(|_| ServerError::Feed)?;
    let mut children = Vec::new();
    for library in &parsed.libraries {
        children.push(RemoteLink::new(&library.name, &library.href)?);
    }
    for row in &parsed.series {
        children.push(RemoteLink::new(&row.name, &row.href)?);
    }
    let mut books = Vec::new();
    for book in &parsed.books {
        books.push(RemoteBook::new(&book.id, &book.name, &book.file)?);
    }
    let progress = RemoteProgress::from_parts(parsed.progress.page, parsed.progress.completed)?;
    Ok(ParsedLibrary::new(
        RemoteCatalog::new(children, books, None),
        progress,
    ))
}

#[derive(Debug, Deserialize)]
struct KavitaJson {
    libraries: Vec<NamedHref>,
    series: Vec<NamedHref>,
    books: Vec<BookRow>,
    progress: ProgressJson,
}

#[derive(Debug, Deserialize)]
struct NamedHref {
    name: String,
    href: String,
}

#[derive(Debug, Deserialize)]
struct BookRow {
    id: String,
    name: String,
    file: String,
}

#[derive(Debug, Deserialize)]
struct ProgressJson {
    page: u64,
    completed: bool,
}

#[cfg(test)]
mod tests {
    use super::parse_kavita;
    use crate::browse::{enqueue_urls, export_auth};
    use crate::komga::parse_komga;
    use crate::AuthConfig;

    #[test]
    fn kavita_maps_into_the_same_browse_types() {
        let kavita =
            parse_kavita(include_str!("../tests/fixtures/kavita/library.json")).expect("kavita");
        let komga = parse_komga(
            include_str!("../tests/fixtures/komga/libraries.json"),
            include_str!("../tests/fixtures/komga/series.json"),
            include_str!("../tests/fixtures/komga/books.json"),
            include_str!("../tests/fixtures/komga/progress.json"),
        )
        .expect("komga");
        let titles = |catalog: &crate::RemoteCatalog| -> Vec<String> {
            catalog
                .children()
                .iter()
                .map(|link| link.title().to_owned())
                .collect()
        };
        assert_eq!(titles(kavita.catalog()), titles(komga.catalog()));
        assert_eq!(
            kavita.catalog().books()[0].title(),
            komga.catalog().books()[0].title()
        );
        assert_eq!(kavita.progress(), komga.progress());

        let mut queued = Vec::new();
        enqueue_urls(kavita.catalog().books(), |url| queued.push(url.to_owned()));
        assert_eq!(queued, ["http://127.0.0.1:9/api/Download/book"]);

        let raw = "raw-api-key";
        let exported = export_auth(&AuthConfig::KavitaSession {
            secret_ref: "keyring:kavita:local".into(),
        })
        .expect("export");
        assert!(exported.contains("keyring:kavita:local"));
        assert!(!exported.contains(raw));
        assert!(!exported.contains("password"));
    }
}
