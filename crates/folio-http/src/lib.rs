//! Loopback page server. Every route requires the process capability token.
#![deny(missing_docs)]

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// Bind address for the page server: loopback, port chosen by the OS.
pub fn loopback_bind() -> SocketAddr {
    SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0))
}

/// Why the page server did not start.
#[derive(Debug, Error)]
pub enum HttpError {
    /// The requested address is not a loopback address.
    #[error("page server only binds to a loopback address")]
    NotLoopback,
    /// The operating system rejected the socket or the token.
    #[error("page server i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// The random token could not be filled.
    #[error("page server could not draw a token: {0}")]
    Random(String),
    /// The loopback origin could not be built from the bound address.
    #[error("page server could not build a loopback url: {0}")]
    Origin(String),
}

/// A running loopback server.
pub struct Running {
    addr: SocketAddr,
    token: Arc<str>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl Running {
    /// Address the server accepted.
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Capability token for this process. Callers send it as `x-folio-token`.
    pub fn token(&self) -> &str {
        &self.token
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            if shutdown.send(()).is_err() {
                eprintln!("folio-http already stopped");
            }
        }
    }
}

/// Serve `body` for every `/page/{id}` on [`loopback_bind`].
///
/// # Errors
///
/// Returns an error when the token cannot be drawn or the socket cannot bind.
pub async fn serve(body: Vec<u8>) -> Result<Running, HttpError> {
    serve_on(loopback_bind(), body).await
}

/// Serve `body` on `addr` when that address is loopback.
///
/// # Errors
///
/// Returns [`HttpError::NotLoopback`] before binding when `addr` is not loopback.
/// Returns an i/o error when the bind fails.
pub async fn serve_on(addr: SocketAddr, body: Vec<u8>) -> Result<Running, HttpError> {
    if !addr.ip().is_loopback() {
        return Err(HttpError::NotLoopback);
    }
    let token = Arc::<str>::from(new_token()?);
    let listener = TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let state = AppState {
        // clone: the request handler and Running share one token allocation
        token: Arc::clone(&token),
        body: Arc::<[u8]>::from(body),
    };
    let app = Router::new()
        .route("/page/{id}", get(page))
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
struct AppState {
    token: Arc<str>,
    body: Arc<[u8]>,
}

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

async fn page(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
) -> Response {
    if id.is_empty() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let presented = header_token(&headers).or(query.token.as_deref());
    let allowed = presented.is_some_and(|value| tokens_match(&state.token, value));
    if !allowed {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    // to_vec: the HTTP response owns a copy of the page bytes
    (StatusCode::OK, state.body.as_ref().to_vec()).into_response()
}

fn header_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-folio-token")
        .and_then(|value| value.to_str().ok())
}

fn tokens_match(expected: &str, given: &str) -> bool {
    let expected = expected.as_bytes();
    let given = given.as_bytes();
    if expected.len() != given.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left, right) in expected.iter().zip(given.iter()) {
        diff |= left ^ right;
    }
    diff == 0
}

fn new_token() -> Result<String, HttpError> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|error| HttpError::Random(error.to_string()))?;
    Ok(hex(&bytes))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

mod library;

pub use library::{serve_library, ServedBook, ServedPage};

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    #[tokio::test]
    async fn rejects_missing_token() {
        let error = super::serve_on("0.0.0.0:9".parse().unwrap(), b"page".to_vec()).await;
        assert!(matches!(error, Err(super::HttpError::NotLoopback)));

        let running = super::serve(b"page-bytes".to_vec()).await.unwrap();
        assert!(running.local_addr().ip().is_loopback());

        let (status, body) = http_get(running.local_addr(), "/page/book-1", None).await;
        assert_eq!(status, 401);
        assert!(body.is_empty());

        let (status, body) =
            http_get(running.local_addr(), "/page/book-1", Some(running.token())).await;
        assert_eq!(status, 200);
        assert_eq!(body, b"page-bytes");
    }

    #[test]
    fn bridge_posts_taps_only_from_the_chrome_document() {
        let source = include_str!("../../../assets/bridge.js");
        assert!(source.contains("dataset.folio"));
        assert!(source.contains("\"chrome\""));
        assert!(source.contains("dioxus.send"));
        assert!(!source.contains("eval"));
    }

    async fn http_get(addr: SocketAddr, path: &str, token: Option<&str>) -> (u16, Vec<u8>) {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let token_header = token
            .map(|token| format!("x-folio-token: {token}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{token_header}\r\n"
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        let split = bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("HTTP response has a header terminator");
        let head = std::str::from_utf8(&bytes[..split]).unwrap();
        let status = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .expect("HTTP status code");
        (status, bytes[split + 4..].to_vec())
    }
}
