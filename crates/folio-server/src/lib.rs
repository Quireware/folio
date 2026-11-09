//! Server connection records and fixture parsers.
//!
//! Parsers return URLs. They do not download bytes, and they never store a raw password.
#![deny(missing_docs)]

mod browse;
mod kavita;
mod kavita_api;
mod kavita_http;
mod komga;
mod librarian;
mod opds;
mod progress;

pub use browse::{
    enqueue_urls, export_auth, parse_progress, ParsedLibrary, RemoteBook, RemoteCatalog,
    RemoteLink, RemoteProgress, ServerError,
};
pub use kavita::parse_kavita;
pub use kavita_api::{
    all_series_request, authed_get, bookmarks_request, collections_request, cover_request,
    dashboard_request, epub_page_request, filters_request, image_page_request, libraries_request,
    mark_chapter_request, mark_series_request, mark_volume_request, on_deck_request, page_len,
    parse_bookmarks, parse_dashboard, parse_filters, parse_grouped_series_ids, parse_libraries,
    parse_series, parse_series_ids, parse_titled, parse_volumes, pdf_request, progress_request,
    reading_lists_request, recently_added_request, recently_updated_request, volumes_request,
    want_to_read_request, KavitaBookmark, KavitaChapter, KavitaLibrary, KavitaNamed,
    KavitaProgress, KavitaRequest, KavitaSeries, KavitaStream, KAVITA_PAGE,
};
pub use kavita_http::execute;
pub use komga::parse_komga;
pub use opds::{authorization_header, parse_opds1, parse_opds2, search_url};
pub use progress::{deliver_progress, progress_write_supported, ProgressFixture};

use thiserror::Error;

/// Stable identifier for a saved server row.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServerId(String);

impl ServerId {
    /// An identifier supplied by storage. This does not allocate a new one.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Borrow the identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Which protocol a connection speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServerKind {
    /// Quireware librarian. Login is a session cookie, not an API key.
    Librarian,
    /// OPDS 2.0 catalog.
    OpdsV2,
    /// OPDS 1.2 catalog.
    OpdsV1,
    /// Komga.
    Komga,
    /// Kavita.
    Kavita,
}

/// Credentials as a reference into the platform secret store.
///
/// The raw password or cookie is never stored on this value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthConfig {
    /// HttpOnly `librarian_session` cookie, addressed by secret-store key.
    LibrarianSession {
        /// Secret-store key, for example `keyring:librarian:local`.
        secret_ref: String,
    },
    /// OPDS HTTP basic credentials, addressed by secret-store key.
    OpdsBasic {
        /// Secret-store key.
        secret_ref: String,
    },
    /// OPDS bearer token, addressed by secret-store key.
    OpdsBearer {
        /// Secret-store key.
        secret_ref: String,
    },
    /// Komga session or basic credentials, addressed by secret-store key.
    KomgaSession {
        /// Secret-store key.
        secret_ref: String,
    },
    /// Kavita credential, addressed by secret-store key.
    KavitaSession {
        /// Secret-store key.
        secret_ref: String,
    },
    /// No credential.
    Anonymous,
}

impl AuthConfig {
    /// Secret-store key, when this arm has one.
    pub fn secret_ref(&self) -> Option<&str> {
        match self {
            Self::LibrarianSession { secret_ref }
            | Self::OpdsBasic { secret_ref }
            | Self::OpdsBearer { secret_ref }
            | Self::KomgaSession { secret_ref }
            | Self::KavitaSession { secret_ref } => Some(secret_ref.as_str()),
            Self::Anonymous => None,
        }
    }
}

/// A saved server. Constructing one does not open a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConnection {
    id: ServerId,
    kind: ServerKind,
    url: String,
    display_name: String,
    auth: AuthConfig,
}

impl ServerConnection {
    /// Record a server. `url` is stored as given; nothing is contacted.
    pub fn new(
        kind: ServerKind,
        url: impl Into<String>,
        display_name: impl Into<String>,
        auth: AuthConfig,
    ) -> Self {
        let display_name = display_name.into();
        Self {
            // clone: the id owns a separate copy of the display name
            id: ServerId::new(display_name.clone()),
            kind,
            url: url.into(),
            display_name,
            auth,
        }
    }

    /// Protocol of this connection.
    pub fn kind(&self) -> ServerKind {
        self.kind
    }

    /// Storage identifier. Phase 1 uses the display name.
    pub fn id(&self) -> &ServerId {
        &self.id
    }

    /// Base URL text.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Name shown in settings.
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// Credential reference.
    pub fn auth(&self) -> &AuthConfig {
        &self.auth
    }

    /// Does not open a socket. Parse a fixture with the protocol modules instead.
    ///
    /// # Errors
    ///
    /// Always returns [`ProtocolNotImplemented`]. A saved row is not a feed.
    pub fn browse(&self) -> Result<(), ProtocolNotImplemented> {
        Err(ProtocolNotImplemented {
            kind: self.kind,
            capability: "browse",
        })
    }
}

/// The named capability is not implemented for this protocol.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{kind:?} does not implement {capability}")]
pub struct ProtocolNotImplemented {
    /// Protocol that was asked.
    pub kind: ServerKind,
    /// Capability name, such as `browse`.
    pub capability: &'static str,
}
