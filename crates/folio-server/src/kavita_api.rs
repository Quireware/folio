//! Kavita 0.9.0.1 request builders and JSON parsers.
//!
//! Paths come from the API document Kover commit `f16bd57` ships. A newer field is ignored.

use std::collections::HashSet;

use serde::Deserialize;
use url::Url;

use crate::ServerError;

/// Rows per page of the series list and Want to Read. Kavita returns every row in one reply
/// when no page size is sent, which outlasts the request timeout on a large library.
pub const KAVITA_PAGE: usize = 100;

/// Rows a shelf (On Deck, recently added, recently updated) asks for.
const SHELF_PAGE: &str = "50";

/// One Kavita request. [`Debug`] does not print the Auth Key.
#[derive(Clone, PartialEq, Eq)]
pub struct KavitaRequest {
    url: Url,
    api_key: String,
    chapter_id: i64,
    page: i64,
    body: Option<String>,
}

impl std::fmt::Debug for KavitaRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("KavitaRequest")
            .field("path", &self.url.path())
            .field("api_key", &"[redacted]")
            .finish()
    }
}

impl KavitaRequest {
    /// Header value. Only `x-api-key` is set.
    pub fn header(&self, name: &str) -> Option<&str> {
        if name.eq_ignore_ascii_case("x-api-key") {
            Some(self.api_key.as_str())
        } else {
            None
        }
    }

    /// Absolute URL, including the query.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Chapter id when this request names one. Otherwise `0`.
    pub fn chapter_id(&self) -> i64 {
        self.chapter_id
    }

    /// Page index when this request names one. Otherwise `0`.
    pub fn page(&self) -> i64 {
        self.page
    }

    /// JSON body for a POST. `None` is a GET.
    pub fn body(&self) -> Option<&str> {
        self.body.as_deref()
    }
}

/// A series row from `SeriesDto`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KavitaSeries {
    /// Series id.
    pub id: i64,
    /// Name.
    pub name: String,
    /// `image`, `archive`, `unknown`, `epub`, or `pdf`.
    pub format: String,
    /// Library id.
    pub library_id: i64,
}

/// A library row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KavitaLibrary {
    /// Library id.
    pub id: i64,
    /// Name.
    pub name: String,
}

/// A chapter inside a volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KavitaChapter {
    /// Chapter id.
    pub id: i64,
    /// Series id.
    pub series_id: i64,
    /// Volume id.
    pub volume_id: i64,
    /// Title, or the chapter number when the title is empty.
    pub name: String,
    /// Page count.
    pub pages: i64,
}

/// One dashboard stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KavitaStream {
    /// Stream id.
    pub id: i64,
    /// Label.
    pub name: String,
    /// `DashboardStreamType`.
    pub stream_type: i64,
    /// Visible on the home screen.
    pub visible: bool,
    /// Sort order.
    pub order: i64,
}

/// A collection, reading list, or smart filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KavitaNamed {
    /// Remote id.
    pub id: i64,
    /// Title or filter name.
    pub name: String,
    /// Encoded filter. Empty for collections and reading lists.
    pub filter: String,
}

/// A page bookmark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KavitaBookmark {
    /// Bookmark id.
    pub id: i64,
    /// Series id.
    pub series_id: i64,
    /// Chapter id.
    pub chapter_id: i64,
    /// Page.
    pub page: i64,
}

/// GET an absolute URL on this server. The Auth Key is the header.
///
/// Image, cover, and PDF paths also receive `apiKey` in the query when it is absent.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when `target` is not an absolute http(s) URL or the key is empty.
pub fn authed_get(target: &str, api_key: &str) -> Result<KavitaRequest, ServerError> {
    if api_key.is_empty() || api_key.chars().any(char::is_whitespace) {
        return Err(ServerError::Feed);
    }
    let mut url = Url::parse(target).map_err(|_| ServerError::Feed)?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(ServerError::Feed);
    }
    let needs_query = url.path().contains("/api/Image/")
        || url.path().contains("/api/Reader/image")
        || url.path().contains("/api/Reader/pdf");
    let has_key = url.query_pairs().any(|(key, _)| key == "apiKey");
    if needs_query && !has_key {
        url.query_pairs_mut().append_pair("apiKey", api_key);
    }
    Ok(KavitaRequest {
        url,
        api_key: api_key.to_owned(),
        chapter_id: 0,
        page: 0,
        body: None,
    })
}

/// Series cover. The key is both the header and the `apiKey` query.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the base is not an absolute URL or the key is empty.
pub fn cover_request(
    base: &str,
    api_key: &str,
    series_id: i64,
) -> Result<KavitaRequest, ServerError> {
    let series = series_id.to_string();
    request(
        base,
        api_key,
        &["api", "Image", "series-cover"],
        &[("seriesId", series.as_str()), ("apiKey", api_key)],
        0,
        0,
        None,
    )
}

/// One EPUB page. `GET /api/Book/{chapterId}/book-page`.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the base is not an absolute URL or the key is empty.
pub fn epub_page_request(
    base: &str,
    api_key: &str,
    chapter_id: i64,
    page: i64,
) -> Result<KavitaRequest, ServerError> {
    let chapter = chapter_id.to_string();
    let page_text = page.to_string();
    request(
        base,
        api_key,
        &["api", "Book", chapter.as_str(), "book-page"],
        &[("page", page_text.as_str())],
        chapter_id,
        page,
        None,
    )
}

/// One comic or archive page. `GET /api/Reader/image`.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the base is not an absolute URL or the key is empty.
pub fn image_page_request(
    base: &str,
    api_key: &str,
    chapter_id: i64,
    page: i64,
) -> Result<KavitaRequest, ServerError> {
    let chapter = chapter_id.to_string();
    let page_text = page.to_string();
    request(
        base,
        api_key,
        &["api", "Reader", "image"],
        &[
            ("chapterId", chapter.as_str()),
            ("page", page_text.as_str()),
            ("apiKey", api_key),
        ],
        chapter_id,
        page,
        None,
    )
}

/// Chapter PDF. `GET /api/Reader/pdf`.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the base is not an absolute URL or the key is empty.
pub fn pdf_request(
    base: &str,
    api_key: &str,
    chapter_id: i64,
) -> Result<KavitaRequest, ServerError> {
    let chapter = chapter_id.to_string();
    request(
        base,
        api_key,
        &["api", "Reader", "pdf"],
        &[("chapterId", chapter.as_str()), ("apiKey", api_key)],
        chapter_id,
        0,
        None,
    )
}

/// `GET /api/Library/libraries`.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the base is not an absolute URL or the key is empty.
pub fn libraries_request(base: &str, api_key: &str) -> Result<KavitaRequest, ServerError> {
    request(
        base,
        api_key,
        &["api", "Library", "libraries"],
        &[],
        0,
        0,
        None,
    )
}

/// `POST /api/Series/all-v2` with an empty filter, for page `page` (from 1) of
/// [`KAVITA_PAGE`] rows.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the base is not an absolute URL or the key is empty.
pub fn all_series_request(
    base: &str,
    api_key: &str,
    page: u32,
) -> Result<KavitaRequest, ServerError> {
    paged_request(base, api_key, &["api", "Series", "all-v2"], page)
}

/// `POST /api/Series/on-deck`.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the base is not an absolute URL or the key is empty.
pub fn on_deck_request(base: &str, api_key: &str) -> Result<KavitaRequest, ServerError> {
    request(
        base,
        api_key,
        &["api", "Series", "on-deck"],
        &[("PageNumber", "1"), ("PageSize", SHELF_PAGE)],
        0,
        0,
        Some("{}".to_owned()),
    )
}

/// `POST /api/Series/recently-added-v2`.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the base is not an absolute URL or the key is empty.
pub fn recently_added_request(base: &str, api_key: &str) -> Result<KavitaRequest, ServerError> {
    request(
        base,
        api_key,
        &["api", "Series", "recently-added-v2"],
        &[("PageNumber", "1"), ("PageSize", SHELF_PAGE)],
        0,
        0,
        Some("{}".to_owned()),
    )
}

/// `POST /api/Series/recently-updated-series`.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the base is not an absolute URL or the key is empty.
pub fn recently_updated_request(base: &str, api_key: &str) -> Result<KavitaRequest, ServerError> {
    request(
        base,
        api_key,
        &["api", "Series", "recently-updated-series"],
        &[("PageNumber", "1"), ("PageSize", SHELF_PAGE)],
        0,
        0,
        Some("{}".to_owned()),
    )
}

/// `GET /api/Series/volumes`.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the base is not an absolute URL or the key is empty.
pub fn volumes_request(
    base: &str,
    api_key: &str,
    series_id: i64,
) -> Result<KavitaRequest, ServerError> {
    let series = series_id.to_string();
    request(
        base,
        api_key,
        &["api", "Series", "volumes"],
        &[("seriesId", series.as_str())],
        0,
        0,
        None,
    )
}

/// `GET /api/Stream/dashboard?visibleOnly=false`.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the base is not an absolute URL or the key is empty.
pub fn dashboard_request(base: &str, api_key: &str) -> Result<KavitaRequest, ServerError> {
    request(
        base,
        api_key,
        &["api", "Stream", "dashboard"],
        &[("visibleOnly", "false")],
        0,
        0,
        None,
    )
}

/// `GET /api/Collection`.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the base is not an absolute URL or the key is empty.
pub fn collections_request(base: &str, api_key: &str) -> Result<KavitaRequest, ServerError> {
    request(base, api_key, &["api", "Collection"], &[], 0, 0, None)
}

/// `POST /api/ReadingList/lists`.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the base is not an absolute URL or the key is empty.
pub fn reading_lists_request(base: &str, api_key: &str) -> Result<KavitaRequest, ServerError> {
    request(
        base,
        api_key,
        &["api", "ReadingList", "lists"],
        &[],
        0,
        0,
        Some("{}".to_owned()),
    )
}

/// `GET /api/Filter`.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the base is not an absolute URL or the key is empty.
pub fn filters_request(base: &str, api_key: &str) -> Result<KavitaRequest, ServerError> {
    request(base, api_key, &["api", "Filter"], &[], 0, 0, None)
}

/// `POST /api/Reader/all-bookmarks` with an empty filter.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the base is not an absolute URL or the key is empty.
pub fn bookmarks_request(base: &str, api_key: &str) -> Result<KavitaRequest, ServerError> {
    request(
        base,
        api_key,
        &["api", "Reader", "all-bookmarks"],
        &[],
        0,
        0,
        Some("{}".to_owned()),
    )
}

/// `POST /api/want-to-read/v2`, for page `page` (from 1) of [`KAVITA_PAGE`] rows.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the base is not an absolute URL or the key is empty.
pub fn want_to_read_request(
    base: &str,
    api_key: &str,
    page: u32,
) -> Result<KavitaRequest, ServerError> {
    paged_request(base, api_key, &["api", "want-to-read", "v2"], page)
}

/// The page a reader has reached inside one chapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KavitaProgress<'a> {
    /// Series id.
    pub series_id: i64,
    /// Volume id.
    pub volume_id: i64,
    /// Chapter id.
    pub chapter_id: i64,
    /// Library id.
    pub library_id: i64,
    /// Zero-based page index.
    pub page: i64,
    /// `lastModifiedUtc` in RFC3339.
    pub updated_rfc3339: &'a str,
}

/// `POST /api/Reader/progress`.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the base is not an absolute URL or the key is empty.
pub fn progress_request(
    base: &str,
    api_key: &str,
    progress: &KavitaProgress<'_>,
) -> Result<KavitaRequest, ServerError> {
    let body = serde_json::json!({
        "volumeId": progress.volume_id,
        "chapterId": progress.chapter_id,
        "pageNum": progress.page,
        "seriesId": progress.series_id,
        "libraryId": progress.library_id,
        "bookScrollId": serde_json::Value::Null,
        "lastModifiedUtc": progress.updated_rfc3339,
    })
    .to_string();
    request(
        base,
        api_key,
        &["api", "Reader", "progress"],
        &[],
        progress.chapter_id,
        progress.page,
        Some(body),
    )
}

/// Chapter read is `POST /api/Reader/mark-chapter-read`.
///
/// Unread is `POST /api/Reader/mark-multiple-unread`. That route's body is
/// `MarkVolumesReadDto`, which includes `chapterIds`. There is no
/// `mark-chapter-unread` route in the pinned document.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the base is not an absolute URL or the key is empty.
pub fn mark_chapter_request(
    base: &str,
    api_key: &str,
    series_id: i64,
    chapter_id: i64,
    read: bool,
) -> Result<KavitaRequest, ServerError> {
    let (segments, body) = if read {
        (
            &["api", "Reader", "mark-chapter-read"][..],
            serde_json::json!({
                "seriesId": series_id,
                "chapterId": chapter_id,
                "generateReadingSession": false,
            })
            .to_string(),
        )
    } else {
        (
            &["api", "Reader", "mark-multiple-unread"][..],
            serde_json::json!({
                "seriesId": series_id,
                "chapterIds": [chapter_id],
            })
            .to_string(),
        )
    };
    request(base, api_key, segments, &[], chapter_id, 0, Some(body))
}

/// Series or volume read and unread.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the base is not an absolute URL or the key is empty.
pub fn mark_series_request(
    base: &str,
    api_key: &str,
    series_id: i64,
    read: bool,
) -> Result<KavitaRequest, ServerError> {
    let leaf = if read { "mark-read" } else { "mark-unread" };
    let body = serde_json::json!({
        "seriesId": series_id,
        "generateReadingSession": false,
    })
    .to_string();
    request(
        base,
        api_key,
        &["api", "Reader", leaf],
        &[],
        0,
        0,
        Some(body),
    )
}

/// Volume read and unread.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the base is not an absolute URL or the key is empty.
pub fn mark_volume_request(
    base: &str,
    api_key: &str,
    series_id: i64,
    volume_id: i64,
    read: bool,
) -> Result<KavitaRequest, ServerError> {
    let leaf = if read {
        "mark-volume-read"
    } else {
        "mark-volume-unread"
    };
    let body = serde_json::json!({
        "seriesId": series_id,
        "volumeId": volume_id,
        "generateReadingSession": false,
    })
    .to_string();
    request(
        base,
        api_key,
        &["api", "Reader", leaf],
        &[],
        0,
        0,
        Some(body),
    )
}

/// Parse a `SeriesDto` array. A row whose name is null or empty is skipped.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the document is not that array.
pub fn parse_series(document: &str) -> Result<Vec<KavitaSeries>, ServerError> {
    let rows: Vec<SeriesJson> = serde_json::from_str(document).map_err(|_| ServerError::Feed)?;
    Ok(rows.into_iter().filter_map(series_from).collect())
}

/// Parse a `SeriesDto` array for its ids alone, in server order.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the document is not an array of objects with an `id`.
pub fn parse_series_ids(document: &str) -> Result<Vec<i64>, ServerError> {
    let rows: Vec<IdJson> = serde_json::from_str(document).map_err(|_| ServerError::Feed)?;
    Ok(rows.into_iter().map(|row| row.id).collect())
}

/// Parse a `GroupedSeriesDto` array into series ids.
///
/// Kavita groups recent updates per series and chapter, so one series can appear more than
/// once; only its first appearance is kept.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the document is not an array of objects with a `seriesId`.
pub fn parse_grouped_series_ids(document: &str) -> Result<Vec<i64>, ServerError> {
    let rows: Vec<GroupedJson> = serde_json::from_str(document).map_err(|_| ServerError::Feed)?;
    let mut seen = HashSet::with_capacity(rows.len());
    Ok(rows
        .into_iter()
        .map(|row| row.series_id)
        .filter(|id| seen.insert(*id))
        .collect())
}

/// Count the rows of a JSON array without reading them, so a page can be judged full even when
/// a parser skips some of its rows.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the document is not an array.
pub fn page_len(document: &str) -> Result<usize, ServerError> {
    let rows: Vec<serde::de::IgnoredAny> =
        serde_json::from_str(document).map_err(|_| ServerError::Feed)?;
    Ok(rows.len())
}

/// Parse a `LibraryDto` array. A row whose name is null or empty is skipped.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the document is not that array.
pub fn parse_libraries(document: &str) -> Result<Vec<KavitaLibrary>, ServerError> {
    let rows: Vec<LibraryJson> = serde_json::from_str(document).map_err(|_| ServerError::Feed)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let name = row.name.filter(|name| !name.is_empty())?;
            Some(KavitaLibrary { id: row.id, name })
        })
        .collect())
}

/// Parse `VolumeDto` array into chapters.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the document is the wrong shape.
pub fn parse_volumes(document: &str, series_id: i64) -> Result<Vec<KavitaChapter>, ServerError> {
    let volumes: Vec<VolumeJson> = serde_json::from_str(document).map_err(|_| ServerError::Feed)?;
    let mut chapters = Vec::new();
    for volume in volumes {
        for chapter in volume.chapters {
            let name = match chapter.title.filter(|title| !title.is_empty()) {
                Some(title) => title,
                None => chapter.number.unwrap_or_default(),
            };
            if name.is_empty() {
                return Err(ServerError::Feed);
            }
            chapters.push(KavitaChapter {
                id: chapter.id,
                series_id,
                volume_id: chapter.volume_id.unwrap_or(volume.id),
                name,
                pages: chapter.pages,
            });
        }
    }
    Ok(chapters)
}

/// Parse dashboard streams.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the document is the wrong shape or a name is empty.
pub fn parse_dashboard(document: &str) -> Result<Vec<KavitaStream>, ServerError> {
    let rows: Vec<StreamJson> = serde_json::from_str(document).map_err(|_| ServerError::Feed)?;
    rows.into_iter()
        .map(|row| {
            if row.name.is_empty() {
                return Err(ServerError::Feed);
            }
            Ok(KavitaStream {
                id: row.id,
                name: row.name,
                stream_type: row.stream_type,
                visible: row.visible,
                order: row.order,
            })
        })
        .collect()
}

/// Parse collections (`title`) or reading lists (`title`).
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the document is the wrong shape or a title is empty.
pub fn parse_titled(document: &str) -> Result<Vec<KavitaNamed>, ServerError> {
    let rows: Vec<TitledJson> = serde_json::from_str(document).map_err(|_| ServerError::Feed)?;
    rows.into_iter()
        .map(|row| {
            if row.title.is_empty() {
                return Err(ServerError::Feed);
            }
            Ok(KavitaNamed {
                id: row.id,
                name: row.title,
                filter: String::new(),
            })
        })
        .collect()
}

/// Parse smart filters.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the document is the wrong shape or a name is empty.
pub fn parse_filters(document: &str) -> Result<Vec<KavitaNamed>, ServerError> {
    let rows: Vec<FilterJson> = serde_json::from_str(document).map_err(|_| ServerError::Feed)?;
    rows.into_iter()
        .map(|row| {
            if row.name.is_empty() {
                return Err(ServerError::Feed);
            }
            Ok(KavitaNamed {
                id: row.id,
                name: row.name,
                filter: row.filter,
            })
        })
        .collect()
}

/// Parse bookmarks.
///
/// # Errors
///
/// Returns [`ServerError::Feed`] when the document is the wrong shape.
pub fn parse_bookmarks(document: &str) -> Result<Vec<KavitaBookmark>, ServerError> {
    let rows: Vec<BookmarkJson> = serde_json::from_str(document).map_err(|_| ServerError::Feed)?;
    Ok(rows
        .into_iter()
        .map(|row| KavitaBookmark {
            id: row.id,
            series_id: row.series_id,
            chapter_id: row.chapter_id,
            page: row.page,
        })
        .collect())
}

fn series_from(row: SeriesJson) -> Option<KavitaSeries> {
    let name = row.name.filter(|name| !name.is_empty())?;
    Some(KavitaSeries {
        id: row.id,
        name,
        format: format_name(row.format).to_owned(),
        library_id: row.library_id,
    })
}

fn format_name(value: i64) -> &'static str {
    match value {
        0 => "image",
        1 => "archive",
        3 => "epub",
        4 => "pdf",
        _ => "unknown",
    }
}

fn paged_request(
    base: &str,
    api_key: &str,
    segments: &[&str],
    page: u32,
) -> Result<KavitaRequest, ServerError> {
    let page = page.to_string();
    let size = KAVITA_PAGE.to_string();
    request(
        base,
        api_key,
        segments,
        &[("PageNumber", &page), ("PageSize", &size)],
        0,
        0,
        Some("{}".to_owned()),
    )
}

fn request(
    base: &str,
    api_key: &str,
    segments: &[&str],
    query: &[(&str, &str)],
    chapter_id: i64,
    page: i64,
    body: Option<String>,
) -> Result<KavitaRequest, ServerError> {
    if api_key.is_empty() || api_key.chars().any(char::is_whitespace) {
        return Err(ServerError::Feed);
    }
    let mut url = Url::parse(base).map_err(|_| ServerError::Feed)?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(ServerError::Feed);
    }
    {
        let mut path = url.path_segments_mut().map_err(|_| ServerError::Feed)?;
        path.pop_if_empty();
        for segment in segments {
            if segment.is_empty() {
                return Err(ServerError::Feed);
            }
            path.push(segment);
        }
    }
    if !query.is_empty() {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in query {
            pairs.append_pair(key, value);
        }
    }
    Ok(KavitaRequest {
        url,
        api_key: api_key.to_owned(),
        chapter_id,
        page,
        body,
    })
}

#[derive(Deserialize)]
struct SeriesJson {
    id: i64,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "libraryId")]
    library_id: i64,
    #[serde(default)]
    format: i64,
}

#[derive(Deserialize)]
struct IdJson {
    id: i64,
}

#[derive(Deserialize)]
struct GroupedJson {
    #[serde(rename = "seriesId")]
    series_id: i64,
}

#[derive(Deserialize)]
struct LibraryJson {
    id: i64,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
struct VolumeJson {
    id: i64,
    #[serde(default)]
    chapters: Vec<ChapterJson>,
}

#[derive(Deserialize)]
struct ChapterJson {
    id: i64,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    number: Option<String>,
    #[serde(default)]
    pages: i64,
    #[serde(default, rename = "volumeId")]
    volume_id: Option<i64>,
}

#[derive(Deserialize)]
struct StreamJson {
    id: i64,
    name: String,
    #[serde(default, rename = "streamType")]
    stream_type: i64,
    #[serde(default = "yes")]
    visible: bool,
    #[serde(default)]
    order: i64,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
struct TitledJson {
    id: i64,
    title: String,
}

#[derive(Deserialize)]
struct FilterJson {
    id: i64,
    name: String,
    #[serde(default)]
    filter: String,
}

#[derive(Deserialize)]
struct BookmarkJson {
    id: i64,
    #[serde(default, rename = "seriesId")]
    series_id: i64,
    #[serde(default, rename = "chapterId")]
    chapter_id: i64,
    #[serde(default)]
    page: i64,
}

#[cfg(test)]
mod tests {
    use super::{
        all_series_request, cover_request, epub_page_request, on_deck_request, page_len,
        parse_grouped_series_ids, parse_libraries, parse_series, parse_series_ids, parse_volumes,
        recently_added_request, recently_updated_request, want_to_read_request,
    };

    const BASE: &str = "http://192.168.1.20:5000";

    #[test]
    fn list_requests_ask_for_one_page() {
        for request in [
            all_series_request(BASE, "raw-api-key", 3),
            want_to_read_request(BASE, "raw-api-key", 3),
        ] {
            let request = request.expect("paged");
            assert_eq!(request.url().query(), Some("PageNumber=3&PageSize=100"));
        }
        for request in [
            on_deck_request(BASE, "raw-api-key"),
            recently_added_request(BASE, "raw-api-key"),
            recently_updated_request(BASE, "raw-api-key"),
        ] {
            let request = request.expect("shelf");
            assert_eq!(request.url().query(), Some("PageNumber=1&PageSize=50"));
        }
    }

    #[test]
    fn page_len_counts_every_row() {
        assert_eq!(
            page_len(r#"[{"id":1,"name":null},{"id":2}]"#).expect("len"),
            2
        );
        assert!(page_len("{}").is_err());
    }

    #[test]
    fn cover_request_sends_the_header_and_the_query_key() {
        let request = cover_request("http://192.168.1.20:5000", "raw-api-key", 45).expect("cover");
        assert_eq!(request.header("x-api-key"), Some("raw-api-key"));
        assert!(request
            .url()
            .query_pairs()
            .any(|(key, value)| key == "apiKey" && value == "raw-api-key"));
        assert!(request.url().path().ends_with("/api/Image/series-cover"));
        assert!(!format!("{request:?}").contains("raw-api-key"));
    }

    #[test]
    fn epub_page_request_is_one_page() {
        let request =
            epub_page_request("http://192.168.1.20:5000/", "raw-api-key", 9, 2).expect("page");
        assert_eq!(request.header("x-api-key"), Some("raw-api-key"));
        assert_eq!(request.chapter_id(), 9);
        assert_eq!(request.page(), 2);
        assert!(request.url().path().ends_with("/book-page"));
    }

    #[test]
    fn series_json_maps_epub_format() {
        let rows =
            parse_series(r#"[{"id":1,"name":"Wings","libraryId":4,"format":3}]"#).expect("series");
        assert_eq!(rows[0].name, "Wings");
        assert_eq!(rows[0].format, "epub");
        assert_eq!(rows[0].library_id, 4);
    }

    #[test]
    fn grouped_series_yield_each_series_id_once_in_order() {
        let document = r#"[
            {"seriesName":"Wings","localizedSeriesName":"","seriesId":5,"libraryId":1,
             "libraryType":2,"created":"2026-11-01T10:00:00","chapterId":40,"volumeId":4,
             "id":1,"format":3,"count":2},
            {"seriesName":"Tide","seriesId":3,"id":2,"chapterId":31,"volumeId":3},
            {"seriesName":"Wings","seriesId":5,"id":3,"chapterId":41,"volumeId":4}
        ]"#;
        assert_eq!(parse_grouped_series_ids(document).expect("grouped"), [5, 3]);
        assert!(parse_grouped_series_ids(r#"[{"id":1}]"#).is_err());
    }

    #[test]
    fn series_ids_need_only_the_id() {
        let ids = parse_series_ids(r#"[{"id":4,"name":null},{"id":6}]"#).expect("ids");
        assert_eq!(ids, [4, 6]);
    }

    #[test]
    fn a_null_chapter_title_or_number_is_read_as_empty() {
        let document = r#"[{"id":2,"chapters":[
            {"id":9,"title":null,"number":"1","pages":10,"volumeId":2},
            {"id":10,"title":"Prologue","number":null,"pages":4}
        ]}]"#;
        let chapters = parse_volumes(document, 7).expect("volumes");
        let names: Vec<&str> = chapters
            .iter()
            .map(|chapter| chapter.name.as_str())
            .collect();
        assert_eq!(names, ["1", "Prologue"]);
        assert_eq!(chapters[1].volume_id, 2);
    }

    #[test]
    fn nameless_series_and_libraries_are_skipped() {
        let series =
            parse_series(r#"[{"id":1,"name":null},{"id":2,"name":""},{"id":3,"name":"Wings"}]"#)
                .expect("series");
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].id, 3);
        let libraries = parse_libraries(r#"[{"id":1,"name":null},{"id":2,"name":"Books"}]"#)
            .expect("libraries");
        assert_eq!(libraries.len(), 1);
        assert_eq!(libraries[0].id, 2);
    }
}
