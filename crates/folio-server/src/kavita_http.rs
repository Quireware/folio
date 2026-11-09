//! Blocking Kavita calls. Rustls verifies certificates unless the caller opts out.
//!
//! The Auth Key is sent as a header. It is not written to the error text.

use std::io::Read;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use ureq::{Agent, Response};

use crate::kavita_api::KavitaRequest;
use crate::ServerError;

/// Longest wait to connect, or for the next bytes of a request or reply. It bounds silence, not
/// the whole request, so a long reply that keeps arriving is not cut off.
const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BODY: u64 = 256 * 1024 * 1024;

/// Perform `request`. A 401 or 403 is [`ServerError::Refused`]. Any other non-success
/// HTTP status is [`ServerError::Feed`]. A name that does not resolve is [`ServerError::Dns`].
/// Any other socket failure is [`ServerError::Transport`].
///
/// `allow_invalid_certs` skips certificate checks. The default caller passes `false`.
///
/// # Errors
///
/// Returns [`ServerError::Secret`] when the request has no Auth Key,
/// [`ServerError::Refused`] when the status is 401 or 403,
/// [`ServerError::Feed`] when another status is not success or the body is too large,
/// [`ServerError::Dns`] when the name does not resolve,
/// and [`ServerError::Transport`] when the host cannot be reached.
pub fn execute(request: &KavitaRequest, allow_invalid_certs: bool) -> Result<Vec<u8>, ServerError> {
    let agent = if allow_invalid_certs {
        insecure_agent()
    } else {
        strict_agent()
    };
    send(agent, request)
}

fn send(agent: &Agent, request: &KavitaRequest) -> Result<Vec<u8>, ServerError> {
    let key = request.header("x-api-key").ok_or(ServerError::Secret)?;
    let method = if request.body().is_some() {
        "POST"
    } else {
        "GET"
    };
    let builder = agent
        .request(method, request.url().as_str())
        .set("x-api-key", key);
    let result = match request.body() {
        Some(body) => builder
            .set("Content-Type", "application/json")
            .send_string(body),
        None => builder.call(),
    };
    match result {
        Ok(response) => read_limited(response),
        Err(ureq::Error::Status(code, _)) if code == 401 || code == 403 => {
            Err(ServerError::Refused)
        }
        Err(ureq::Error::Status(_, _)) => Err(ServerError::Feed),
        Err(ureq::Error::Transport(err)) if err.kind() == ureq::ErrorKind::Dns => {
            Err(ServerError::Dns)
        }
        Err(_) => Err(ServerError::Transport),
    }
}

fn read_limited(response: Response) -> Result<Vec<u8>, ServerError> {
    let limit = usize::try_from(MAX_BODY).unwrap_or(usize::MAX);
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_BODY.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| ServerError::Transport)?;
    if bytes.len() > limit {
        return Err(ServerError::Feed);
    }
    Ok(bytes)
}

fn strict_agent() -> &'static Agent {
    static AGENT: OnceLock<Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout_connect(TIMEOUT)
            .timeout_read(TIMEOUT)
            .timeout_write(TIMEOUT)
            .redirects(0)
            .build()
    })
}

fn insecure_agent() -> &'static Agent {
    static AGENT: OnceLock<Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        let config = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_custom_certificate_verifier(Arc::new(AcceptInvalidCerts))
            .with_no_client_auth();
        ureq::AgentBuilder::new()
            .timeout_connect(TIMEOUT)
            .timeout_read(TIMEOUT)
            .timeout_write(TIMEOUT)
            .redirects(0)
            .tls_config(Arc::new(config))
            .build()
    })
}

struct AcceptInvalidCerts;

impl rustls::client::ServerCertVerifier for AcceptInvalidCerts {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use super::execute;
    use crate::{epub_page_request, libraries_request, ServerError};

    #[test]
    fn loopback_epub_page_sends_the_key_header() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buffer = [0u8; 4096];
            let count = stream.read(&mut buffer).expect("read");
            let text = String::from_utf8_lossy(&buffer[..count]);
            assert!(text.to_ascii_lowercase().contains("x-api-key: raw-api-key"));
            assert!(!text.contains("apiKey="));
            let body = "<p>page</p>";
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).expect("head");
            stream.write_all(body.as_bytes()).expect("body");
        });
        let mut base = url::Url::parse("http://127.0.0.1/").expect("url");
        base.set_port(Some(port)).expect("port");
        let request = epub_page_request(base.as_str(), "raw-api-key", 9, 0).expect("request");
        let bytes = execute(&request, false).expect("execute");
        assert_eq!(bytes, b"<p>page</p>");
        handle.join().expect("server");
    }

    #[test]
    fn a_closed_port_is_transport_and_hides_the_key() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);
        let mut base = url::Url::parse("http://127.0.0.1/").expect("url");
        base.set_port(Some(port)).expect("port");
        let request = libraries_request(base.as_str(), "raw-api-key").expect("request");
        let error = execute(&request, false).expect_err("closed");
        assert!(matches!(error, ServerError::Transport));
        assert!(!error.to_string().contains("raw-api-key"));
    }

    #[test]
    fn a_refused_key_is_not_copied_into_the_error() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buffer = [0u8; 1024];
            let _count = stream.read(&mut buffer);
            stream
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("write");
        });
        let mut base = url::Url::parse("http://127.0.0.1/").expect("url");
        base.set_port(Some(port)).expect("port");
        let request = libraries_request(base.as_str(), "raw-api-key").expect("request");
        let error = execute(&request, false).expect_err("refused");
        assert!(matches!(error, ServerError::Refused));
        assert!(!error.to_string().contains("raw-api-key"));
        handle.join().expect("server");
    }
}
