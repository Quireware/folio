use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn test_image_returns_fixture_png() {
    let app = folio_spike::server::router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/test-image")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").map(|v| v.as_bytes()),
        Some(&b"image/png"[..])
    );
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    assert_eq!(bytes.as_ref(), include_bytes!("../assets/test-image.png"));
}
