//! Shared browse records. Parsers hand the caller a URL. They do not download bytes.

use crate::AuthConfig;
use thiserror::Error;

/// Why a fixture or credential could not be used.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ServerError {
    /// The fixture was missing a required field or was not valid JSON or XML.
    #[error("the server fixture could not be read")]
    Feed,
    /// The search link has no `{searchTerms}` placeholder.
    #[error("the search template has no searchTerms placeholder")]
    Template,
    /// The secret reference is empty or cannot sit in a header.
    #[error("the secret reference is empty")]
    Secret,
    /// The server answered 401 or 403. The Auth Key was not accepted.
    #[error("the server refused the Auth Key")]
    Refused,
    /// This protocol has no progress-write route.
    #[error("that server has no progress write")]
    Unsupported,
    /// The TCP request did not complete.
    #[error("the server could not be reached")]
    Transport,
    /// The server name did not resolve. The name is not included.
    #[error("the server name could not be found")]
    Dns,
}

/// One navigation link, in feed order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteLink {
    title: String,
    href: String,
}

impl RemoteLink {
    pub(crate) fn new(title: &str, href: &str) -> Result<Self, ServerError> {
        if title.is_empty() || href.is_empty() {
            return Err(ServerError::Feed);
        }
        Ok(Self {
            // to_owned: the catalog keeps the title after the fixture is dropped
            title: title.to_owned(),
            href: href.to_owned(),
        })
    }

    /// Label shown for this link.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Target URL or path from the fixture.
    pub fn href(&self) -> &str {
        &self.href
    }
}

/// A book the caller can queue. `download_url` is not fetched here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteBook {
    id: String,
    title: String,
    download_url: String,
}

impl RemoteBook {
    pub(crate) fn new(id: &str, title: &str, download_url: &str) -> Result<Self, ServerError> {
        if id.is_empty() || title.is_empty() || download_url.is_empty() {
            return Err(ServerError::Feed);
        }
        Ok(Self {
            // to_owned: the book row outlives the fixture borrow
            id: id.to_owned(),
            title: title.to_owned(),
            download_url: download_url.to_owned(),
        })
    }

    /// Stable id from the fixture.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Title from the fixture.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Acquisition URL. The caller queues this. This crate does not read the body.
    pub fn download_url(&self) -> &str {
        &self.download_url
    }
}

/// A navigation feed and the books directly inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCatalog {
    children: Vec<RemoteLink>,
    books: Vec<RemoteBook>,
    search_template: Option<String>,
}

impl RemoteCatalog {
    pub(crate) fn new(
        children: Vec<RemoteLink>,
        books: Vec<RemoteBook>,
        search_template: Option<String>,
    ) -> Self {
        Self {
            children,
            books,
            search_template,
        }
    }

    /// Child links in fixture order.
    pub fn children(&self) -> &[RemoteLink] {
        &self.children
    }

    /// Books with acquisition URLs.
    pub fn books(&self) -> &[RemoteBook] {
        &self.books
    }

    /// Search template containing `{searchTerms}`, when the feed has one.
    pub fn search_template(&self) -> Option<&str> {
        self.search_template.as_deref()
    }
}

/// A read-only progress position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteProgress {
    page: u32,
    complete: bool,
}

impl RemoteProgress {
    pub(crate) fn from_parts(page: u64, complete: bool) -> Result<Self, ServerError> {
        let page = u32::try_from(page).map_err(|_| ServerError::Feed)?;
        Ok(Self { page, complete })
    }

    /// Page index from the server.
    pub fn page(self) -> u32 {
        self.page
    }

    /// Whether the server marked the book finished.
    pub fn is_complete(self) -> bool {
        self.complete
    }
}

/// Library browse plus the progress read that came with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLibrary {
    catalog: RemoteCatalog,
    progress: RemoteProgress,
}

impl ParsedLibrary {
    pub(crate) fn new(catalog: RemoteCatalog, progress: RemoteProgress) -> Self {
        Self { catalog, progress }
    }

    /// Libraries, series, and books.
    pub fn catalog(&self) -> &RemoteCatalog {
        &self.catalog
    }

    /// Progress read. This is not a write.
    pub fn progress(&self) -> RemoteProgress {
        self.progress
    }
}

/// Pass each acquisition URL to `sink`. The sink receives a URL, never file bytes.
pub fn enqueue_urls<'a>(
    books: impl IntoIterator<Item = &'a RemoteBook>,
    mut sink: impl FnMut(&str),
) {
    for book in books {
        sink(book.download_url());
    }
}

/// JSON for a saved credential. The value is the secret-store key, never a password field.
///
/// # Errors
///
/// Returns [`ServerError::Secret`] when a reference is empty, and [`ServerError::Feed`]
/// when the JSON writer fails.
pub fn export_auth(auth: &AuthConfig) -> Result<String, ServerError> {
    let value = match auth {
        AuthConfig::LibrarianSession { secret_ref } => json_secret("librarian", secret_ref)?,
        AuthConfig::OpdsBasic { secret_ref } => json_secret("basic", secret_ref)?,
        AuthConfig::OpdsBearer { secret_ref } => json_secret("bearer", secret_ref)?,
        AuthConfig::KomgaSession { secret_ref } => json_secret("komga", secret_ref)?,
        AuthConfig::KavitaSession { secret_ref } => json_secret("kavita", secret_ref)?,
        AuthConfig::Anonymous => serde_json::json!({"scheme": "anonymous"}),
    };
    serde_json::to_string(&value).map_err(|_| ServerError::Feed)
}

fn json_secret(scheme: &str, secret_ref: &str) -> Result<serde_json::Value, ServerError> {
    if secret_ref.is_empty() {
        return Err(ServerError::Secret);
    }
    Ok(serde_json::json!({ "scheme": scheme, "secret_ref": secret_ref }))
}

/// Read `{page, completed}` progress JSON.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the document is not that object or the page does not fit in `u32`.
pub fn parse_progress(document: &str) -> Result<RemoteProgress, ServerError> {
    let parsed: ProgressJson = serde_json::from_str(document).map_err(|_| ServerError::Feed)?;
    RemoteProgress::from_parts(parsed.page, parsed.completed)
}

#[derive(Debug, serde::Deserialize)]
struct ProgressJson {
    page: u64,
    completed: bool,
}
