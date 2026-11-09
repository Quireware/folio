//! OPDS 2.0 and OPDS 1.2 fixture parsers.
//!
//! Acquisition links are URLs. Callers pass them to the download queue. This module does not fetch them.

use crate::browse::{RemoteBook, RemoteCatalog, RemoteLink, ServerError};
use crate::AuthConfig;
use roxmltree::Node;
use serde::Deserialize;
use url::Url;

/// Parse an OPDS 2.0 navigation document.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the JSON is not an OPDS 2.0 feed or a publication has no acquisition link.
pub fn parse_opds2(document: &str) -> Result<RemoteCatalog, ServerError> {
    let feed: FeedJson = serde_json::from_str(document).map_err(|_| ServerError::Feed)?;
    let mut children = Vec::new();
    for item in feed.navigation.unwrap_or_default() {
        children.push(RemoteLink::new(&item.title, &item.href)?);
    }
    let mut books = Vec::new();
    for publication in feed.publications.unwrap_or_default() {
        let href = publication
            .links
            .iter()
            .find(|link| link.rel.contains("acquisition"))
            .map(|link| link.href.as_str())
            .ok_or(ServerError::Feed)?;
        let book_id = publication
            .metadata
            .identifier
            .as_deref()
            .filter(|id| !id.is_empty())
            .unwrap_or(publication.metadata.title.as_str());
        books.push(RemoteBook::new(book_id, &publication.metadata.title, href)?);
    }
    let search_template = feed.links.unwrap_or_default().into_iter().find_map(|link| {
        if link.rel == "search" && !link.href.is_empty() {
            Some(link.href)
        } else {
            None
        }
    });
    Ok(RemoteCatalog::new(children, books, search_template))
}

/// Parse an OPDS 1.2 Atom navigation document.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the XML is not a feed or an entry is neither navigation nor an acquisition.
pub fn parse_opds1(document: &str) -> Result<RemoteCatalog, ServerError> {
    let doc = roxmltree::Document::parse(document).map_err(|_| ServerError::Feed)?;
    let root = doc.root_element();
    if root.tag_name().name() != "feed" {
        return Err(ServerError::Feed);
    }
    let mut children = Vec::new();
    let mut books = Vec::new();
    let mut search_template = None;
    for node in root.children().filter(Node::is_element) {
        match node.tag_name().name() {
            "link" if node.attribute("rel") == Some("search") => {
                let href = node.attribute("href").ok_or(ServerError::Feed)?;
                if href.is_empty() {
                    return Err(ServerError::Feed);
                }
                // to_owned: the catalog keeps the template after the XML document is dropped
                search_template = Some(href.to_owned());
            }
            "entry" => classify_entry(node, &mut children, &mut books)?,
            _ => {}
        }
    }
    Ok(RemoteCatalog::new(children, books, search_template))
}

/// Expand `{searchTerms}` with `Url` query pairs. This does not build the URL with `format!`.
///
/// # Errors
///
/// Returns [`ServerError::Template`] when the template has no `{searchTerms}` placeholder or the base is not a URL.
pub fn search_url(template: &str, terms: &str) -> Result<Url, ServerError> {
    let (before, after) = template
        .split_once("{searchTerms}")
        .ok_or(ServerError::Template)?;
    if !after.is_empty() || before.contains('{') {
        return Err(ServerError::Template);
    }
    let (base, query) = before.split_once('?').ok_or(ServerError::Template)?;
    let key = query.strip_suffix('=').ok_or(ServerError::Template)?;
    if key.is_empty() || key.contains('&') || key.contains('=') {
        return Err(ServerError::Template);
    }
    let mut url = Url::parse(base).map_err(|_| ServerError::Template)?;
    url.query_pairs_mut().append_pair(key, terms);
    Ok(url)
}

/// `Authorization` header from a secret-store key. The raw password is not an argument.
///
/// # Errors
///
/// Returns [`ServerError::Secret`] when the secret reference is empty or contains header whitespace.
pub fn authorization_header(auth: &AuthConfig) -> Result<Option<String>, ServerError> {
    match auth {
        AuthConfig::OpdsBasic { secret_ref } => Ok(Some(header_value("Basic", secret_ref)?)),
        AuthConfig::OpdsBearer { secret_ref } => Ok(Some(header_value("Bearer", secret_ref)?)),
        AuthConfig::LibrarianSession { .. }
        | AuthConfig::KomgaSession { .. }
        | AuthConfig::KavitaSession { .. }
        | AuthConfig::Anonymous => Ok(None),
    }
}

fn header_value(scheme: &str, secret_ref: &str) -> Result<String, ServerError> {
    if secret_ref.is_empty()
        || secret_ref
            .chars()
            .any(|character| character.is_whitespace() || character == '"' || character == ',')
    {
        return Err(ServerError::Secret);
    }
    Ok(format!("{scheme} {secret_ref}"))
}

fn classify_entry(
    entry: Node<'_, '_>,
    children: &mut Vec<RemoteLink>,
    books: &mut Vec<RemoteBook>,
) -> Result<(), ServerError> {
    let title = element_text(entry, "title").ok_or(ServerError::Feed)?;
    let id = element_text(entry, "id");
    let mut acquisition = None;
    let mut subsection = None;
    for node in entry
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "link")
    {
        let rel = node.attribute("rel").unwrap_or("");
        let href = node.attribute("href").ok_or(ServerError::Feed)?;
        if href.is_empty() {
            return Err(ServerError::Feed);
        }
        if rel.contains("acquisition") {
            acquisition = Some(href);
        } else if rel.contains("subsection") {
            subsection = Some(href);
        }
    }
    if let Some(href) = acquisition {
        let book_id = id.unwrap_or(title);
        books.push(RemoteBook::new(book_id, title, href)?);
    } else if let Some(href) = subsection {
        children.push(RemoteLink::new(title, href)?);
    } else {
        return Err(ServerError::Feed);
    }
    Ok(())
}

fn element_text<'a>(node: Node<'a, 'a>, name: &str) -> Option<&'a str> {
    node.children()
        .find(|child| child.is_element() && child.tag_name().name() == name)
        .and_then(|child| child.text())
        .filter(|text| !text.is_empty())
}

#[derive(Debug, Deserialize)]
struct FeedJson {
    links: Option<Vec<LinkJson>>,
    navigation: Option<Vec<NavJson>>,
    publications: Option<Vec<PublicationJson>>,
}

#[derive(Debug, Deserialize)]
struct LinkJson {
    rel: String,
    href: String,
}

#[derive(Debug, Deserialize)]
struct NavJson {
    title: String,
    href: String,
}

#[derive(Debug, Deserialize)]
struct PublicationJson {
    metadata: MetaJson,
    links: Vec<LinkJson>,
}

#[derive(Debug, Deserialize)]
struct MetaJson {
    title: String,
    identifier: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{authorization_header, parse_opds1, parse_opds2, search_url};
    use crate::browse::{enqueue_urls, export_auth};
    use crate::{AuthConfig, ServerError};

    #[test]
    fn opds_feeds_search_auth_and_acquisition() {
        let opds2 =
            parse_opds2(include_str!("../tests/fixtures/opds/catalog.json")).expect("opds2");
        let opds1 = parse_opds1(include_str!("../tests/fixtures/opds/catalog.xml")).expect("opds1");
        let hrefs = |catalog: &crate::RemoteCatalog| -> Vec<String> {
            catalog
                .children()
                .iter()
                .map(|link| link.href().to_owned())
                .collect()
        };
        assert_eq!(
            hrefs(&opds2),
            [
                "http://127.0.0.1:9/fiction".to_owned(),
                "http://127.0.0.1:9/comics".to_owned()
            ]
        );
        assert_eq!(hrefs(&opds1), hrefs(&opds2));

        let template = opds2.search_template().expect("template");
        assert_eq!(opds1.search_template(), Some(template));
        let url = search_url(template, "dune mesa").expect("search");
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        assert_eq!(pairs, [("q".to_owned(), "dune mesa".to_owned())]);
        assert_eq!(
            search_url("http://127.0.0.1:9/search", "dune"),
            Err(ServerError::Template)
        );

        let raw = "s3cret-password";
        let basic = AuthConfig::OpdsBasic {
            secret_ref: "keyring:opds:basic".into(),
        };
        let bearer = AuthConfig::OpdsBearer {
            secret_ref: "keyring:opds:bearer".into(),
        };
        assert_eq!(
            authorization_header(&basic).expect("basic"),
            Some("Basic keyring:opds:basic".to_owned())
        );
        assert_eq!(
            authorization_header(&bearer).expect("bearer"),
            Some("Bearer keyring:opds:bearer".to_owned())
        );
        for auth in [
            basic,
            bearer,
            AuthConfig::LibrarianSession {
                secret_ref: "keyring:librarian:local".into(),
            },
            AuthConfig::Anonymous,
        ] {
            let exported = export_auth(&auth).expect("export");
            assert!(!exported.contains(raw), "{exported}");
            assert!(!exported.contains("password"), "{exported}");
        }

        let mut queued = Vec::new();
        enqueue_urls(opds2.books(), |url| queued.push(url.to_owned()));
        assert_eq!(queued, ["http://127.0.0.1:9/dune.epub"]);
        assert_eq!(opds2.books()[0].id(), "dune");
    }
}
