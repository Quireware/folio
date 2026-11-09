// Rust guideline compliant 2026-09-09
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use thiserror::Error;
use url::Url;

const TEST_IMAGE: &[u8] = include_bytes!("../assets/test-image.png");

#[derive(Debug, Error)]
pub(crate) enum ImageUrlError {
    #[error("loopback image URL is invalid")]
    InvalidBase(#[from] url::ParseError),
    #[error("loopback image URL cannot use the selected port")]
    InvalidPort,
}

pub fn router() -> Router {
    Router::new().route("/test-image", get(test_image))
}

async fn test_image() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/png")],
        TEST_IMAGE,
    )
        .into_response()
}

pub(crate) fn test_image_url(port: u16) -> Result<Url, ImageUrlError> {
    let mut url = Url::parse("http://127.0.0.1/test-image")?;
    url.set_port(Some(port))
        .map_err(|()| ImageUrlError::InvalidPort)?;
    Ok(url)
}

pub(crate) async fn bind_and_serve() -> Result<Url, ServerError> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let image_url = test_image_url(listener.local_addr()?.port())?;
    let app = router();
    tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            eprintln!("folio-spike http: {error}");
        }
    });
    Ok(image_url)
}

#[derive(Debug, Error)]
pub(crate) enum ServerError {
    #[error("local server I/O failed")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    ImageUrl(#[from] ImageUrlError),
}

#[cfg(test)]
mod tests {
    use super::test_image_url;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn test_image_url_is_loopback_http(port in any::<u16>()) {
            let url = test_image_url(port).expect("the fixed loopback base URL should be valid");

            prop_assert_eq!(url.scheme(), "http");
            prop_assert_eq!(url.host_str(), Some("127.0.0.1"));
            prop_assert_eq!(url.port_or_known_default(), Some(port));
            prop_assert_eq!(url.path(), "/test-image");
        }
    }
}
