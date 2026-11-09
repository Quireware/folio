//! Tokenised page and chapter routes.

use std::collections::HashMap;
use std::net::SocketAddr;

use folio_http::{serve_library, ServedBook, ServedPage};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::test]
async fn page_requires_token_and_honours_range() {
    let mut pages = vec![0u8; 64];
    pages[0] = 0xff;
    pages[15] = 0x10;
    let mut books = HashMap::new();
    books.insert(
        "book-1".to_owned(),
        ServedBook {
            pages: vec![ServedPage {
                content_type: "image/jpeg".to_owned(),
                bytes: pages,
            }],
            chapters: vec![chapter_html()],
        },
    );
    let running = serve_library(books).await.expect("server");
    let addr = running.local_addr();
    let token = running.token();

    let denied = request(addr, "/page/book-1/0", None, None).await;
    assert_eq!(denied.status, 401);
    assert!(denied.body.is_empty());

    let page = request(addr, "/page/book-1/0", Some(token), None).await;
    assert_eq!(page.status, 200);
    assert_eq!(page.header("content-type"), Some("image/jpeg"));
    assert_eq!(page.header("accept-ranges"), Some("bytes"));
    assert_eq!(page.body.len(), 64);
    assert_eq!(page.body[0], 0xff);

    let range = request(addr, "/page/book-1/0", Some(token), Some("bytes=0-15")).await;
    assert_eq!(range.status, 206);
    assert_eq!(range.header("content-range"), Some("bytes 0-15/64"));
    assert_eq!(range.body.len(), 16);
    assert_eq!(range.body[0], 0xff);
    assert_eq!(range.body[15], 0x10);

    let bad = request(addr, "/page/book-1/0", Some(token), Some("bytes=80-90")).await;
    assert_eq!(bad.status, 416);

    let missing = request(addr, "/page/missing/0", Some(token), None).await;
    assert_eq!(missing.status, 404);

    let chapter = request(addr, "/chapter/book-1/0", Some(token), None).await;
    assert_eq!(chapter.status, 200);
    let body = String::from_utf8(chapter.body).expect("utf-8 chapter");
    let folded = body.to_ascii_lowercase();
    assert!(body.contains("127.0.0.1"), "{body}");
    assert!(body.contains(token), "{body}");
    assert!(body.contains("/page/book-1/0"), "{body}");
    assert!(folded.contains("hello"));
    assert!(!folded.contains("<script"), "{body}");
    assert!(!folded.contains("onclick"), "{body}");
    assert!(!folded.contains("<form"), "{body}");
    assert!(!folded.contains("example.test"), "{body}");
    assert!(!folded.contains("dioxus"), "{body}");
    assert!(!folded.contains("addjavascriptinterface"), "{body}");
    assert!(!folded.contains("dataset.folio"), "{body}");
}

fn chapter_html() -> String {
    r#"<p onclick="alert(1)">Hello</p><img src="pic.png"><img src="https://example.test/a.png"><script>bad()</script><form><input></form>"#.to_owned()
}

struct Reply {
    status: u16,
    head: String,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.head.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then_some(value.trim())
        })
    }
}

async fn request(addr: SocketAddr, path: &str, token: Option<&str>, range: Option<&str>) -> Reply {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let token_header = token
        .map(|token| format!("x-folio-token: {token}\r\n"))
        .unwrap_or_default();
    let range_header = range
        .map(|range| format!("Range: {range}\r\n"))
        .unwrap_or_default();
    let message = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{token_header}{range_header}\r\n"
    );
    stream.write_all(message.as_bytes()).await.expect("write");
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.expect("read");
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("header terminator");
    let head = String::from_utf8(bytes[..split].to_vec()).expect("header utf-8");
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("status");
    Reply {
        status,
        head,
        body: bytes[split + 4..].to_vec(),
    }
}
