//! Tokenised page and chapter routes. Chapter HTML is sanitized before it is sent.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::header::{self, HeaderMap, HeaderValue};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use url::Url;

use crate::{header_token, new_token, tokens_match, HttpError, Running};

/// One image the page route can stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServedPage {
    /// `Content-Type` value, such as `image/jpeg`.
    pub content_type: String,
    /// Encoded image bytes.
    pub bytes: Vec<u8>,
}

/// Pages and chapter HTML for one book.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServedBook {
    /// Zero-based pages.
    pub pages: Vec<ServedPage>,
    /// Chapter HTML before sanitizing. Index matches the chapter route.
    pub chapters: Vec<String>,
}

/// Serve `books` on loopback. Every route requires the capability token.
///
/// # Errors
///
/// Returns an error when the token or the loopback URL cannot be built, or the socket cannot bind.
pub async fn serve_library(books: HashMap<String, ServedBook>) -> Result<Running, HttpError> {
    let addr = crate::loopback_bind();
    if !addr.ip().is_loopback() {
        return Err(HttpError::NotLoopback);
    }
    let token = Arc::<str>::from(new_token()?);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    let origin = loopback_origin(bound)?;
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let state = LibraryState {
        token: Arc::clone(&token),
        origin,
        books: Arc::new(books),
    };
    let app = Router::new()
        .route("/page/{book}/{index}", get(page))
        .route("/chapter/{book}/{index}", get(chapter))
        .with_state(state);
    tokio::spawn(async move {
        let server = axum::serve(listener, app).with_graceful_shutdown(async move {
            match shutdown_rx.await {
                Ok(()) => {}
                Err(error) => eprintln!("folio-http shutdown closed: {error}"),
            }
        });
        if let Err(error) = server.await {
            eprintln!("folio-http stopped: {error}");
        }
    });
    Ok(Running {
        addr: bound,
        token,
        shutdown: Some(shutdown_tx),
    })
}

#[derive(Clone)]
struct LibraryState {
    token: Arc<str>,
    origin: Url,
    books: Arc<HashMap<String, ServedBook>>,
}

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

async fn page(
    State(state): State<LibraryState>,
    Path((book, index)): Path<(String, u32)>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
) -> Response {
    if !authorized(&state, &headers, query.token.as_deref()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(page) = state.books.get(&book).and_then(|book| {
        usize::try_from(index)
            .ok()
            .and_then(|index| book.pages.get(index))
    }) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let ranged = match slice_range(&page.bytes, headers.get(header::RANGE)) {
        Ok(ranged) => ranged,
        Err(status) => return status.into_response(),
    };
    bytes_response(&page.content_type, ranged)
}

async fn chapter(
    State(state): State<LibraryState>,
    Path((book, index)): Path<(String, u32)>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
) -> Response {
    if !authorized(&state, &headers, query.token.as_deref()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(html) = state.books.get(&book).and_then(|book| {
        usize::try_from(index)
            .ok()
            .and_then(|index| book.chapters.get(index))
    }) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(resource) = page_url(&state.origin, &book, 0, &state.token) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let clean = folio_formats::sanitize_chapter_html(html);
    let rewritten = rewrite_resources(&clean, resource.as_str());
    bytes_response(
        "text/html; charset=utf-8",
        Ranged {
            status: StatusCode::OK,
            body: rewritten.into_bytes(),
            content_range: None,
        },
    )
}

fn authorized(state: &LibraryState, headers: &HeaderMap, query: Option<&str>) -> bool {
    let presented = header_token(headers).or(query);
    presented.is_some_and(|value| tokens_match(&state.token, value))
}

struct Ranged {
    status: StatusCode,
    body: Vec<u8>,
    content_range: Option<String>,
}

fn slice_range(bytes: &[u8], header: Option<&HeaderValue>) -> Result<Ranged, StatusCode> {
    let Some(header) = header else {
        return Ok(Ranged {
            status: StatusCode::OK,
            // to_vec: the HTTP response owns the page bytes
            body: bytes.to_vec(),
            content_range: None,
        });
    };
    let Ok(text) = header.to_str() else {
        return Err(StatusCode::RANGE_NOT_SATISFIABLE);
    };
    let Some((start, end)) = parse_byte_range(text, bytes.len()) else {
        return Err(StatusCode::RANGE_NOT_SATISFIABLE);
    };
    let body = bytes
        .get(start..=end)
        .ok_or(StatusCode::RANGE_NOT_SATISFIABLE)?
        // to_vec: the HTTP response owns this range
        .to_vec();
    let total = bytes.len();
    Ok(Ranged {
        status: StatusCode::PARTIAL_CONTENT,
        content_range: Some(format!("bytes {start}-{end}/{total}")),
        body,
    })
}

fn parse_byte_range(header: &str, len: usize) -> Option<(usize, usize)> {
    let rest = header.trim().strip_prefix("bytes=")?;
    if rest.contains(',') {
        return None;
    }
    let (start_text, end_text) = rest.split_once('-')?;
    if start_text.is_empty() || end_text.is_empty() {
        return None;
    }
    let start: usize = start_text.parse().ok()?;
    let end: usize = end_text.parse().ok()?;
    if start > end || start >= len {
        return None;
    }
    let last = len.saturating_sub(1);
    Some((start, end.min(last)))
}

fn bytes_response(content_type: &str, ranged: Ranged) -> Response {
    let mut builder = Response::builder().status(ranged.status);
    if let Ok(value) = HeaderValue::from_str(content_type) {
        builder = builder.header(header::CONTENT_TYPE, value);
    }
    builder = builder.header(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Some(range) = ranged.content_range {
        if let Ok(value) = HeaderValue::from_str(&range) {
            builder = builder.header(header::CONTENT_RANGE, value);
        }
    }
    match builder.body(axum::body::Body::from(ranged.body)) {
        Ok(response) => response,
        Err(error) => {
            eprintln!("folio-http could not build a response: {error}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn loopback_origin(addr: SocketAddr) -> Result<Url, HttpError> {
    let mut url =
        Url::parse("http://127.0.0.1/").map_err(|error| HttpError::Origin(error.to_string()))?;
    url.set_port(Some(addr.port()))
        .map_err(|()| HttpError::Origin("loopback port was rejected".to_owned()))?;
    url.set_path("/");
    Ok(url)
}

fn page_url(origin: &Url, book: &str, index: u32, token: &str) -> Option<Url> {
    if book.is_empty() || book.contains(['/', '?', '#', ' ']) {
        return None;
    }
    // clone: the server keeps `origin`; this URL is one response link
    let mut url = origin.clone();
    {
        let mut segments = url.path_segments_mut().ok()?;
        segments.clear();
        segments.push("page");
        segments.push(book);
        let index_text = index.to_string();
        segments.push(&index_text);
    }
    url.query_pairs_mut().clear().append_pair("token", token);
    Some(url)
}

fn rewrite_resources(html: &str, page: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = find_local_attribute(rest) {
        let (before, value_and_after) = rest.split_at(start);
        out.push_str(before);
        let Some(end) = value_and_after.find('"') else {
            out.push_str(value_and_after);
            break;
        };
        let Some(&quote) = value_and_after.as_bytes().get(end) else {
            out.push_str(value_and_after);
            break;
        };
        let value = &value_and_after[..end];
        if value.contains(':') {
            out.push_str(value);
        } else {
            out.push_str(page);
        }
        out.push(char::from(quote));
        rest = &value_and_after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn find_local_attribute(html: &str) -> Option<usize> {
    let folded = html.to_ascii_lowercase();
    let src = folded.find("src=\"").map(|index| index + 5);
    let href = folded.find("href=\"").map(|index| index + 6);
    match (src, href) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(index), None) | (None, Some(index)) => Some(index),
        (None, None) => None,
    }
}
