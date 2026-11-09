//! Download queue. The worker runs only while the download screen is open.
//!
//! There is no Android service type in this module.

use std::fs;
use std::io::Error as IoError;
use std::path::Path;

use folio_formats::file_extension;
use folio_storage::{
    downloads, enqueue_download, set_download_status, CatalogItem, Database, DownloadRow,
    StorageError,
};
use thiserror::Error;

/// Why a download did not run.
#[derive(Debug, Error)]
pub enum DownloadError {
    /// The URL is already queued.
    #[error("that url is already queued")]
    Duplicate,
    /// Wi-Fi-only is set and the network is metered.
    #[error("wifi-only refused a metered network")]
    Metered,
    /// The URL is not a loopback fixture.
    #[error("download url must be loopback")]
    Host,
    /// Storage failed.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// The file could not be written.
    #[error(transparent)]
    Io(#[from] IoError),
}

/// Queue `url` unless that URL is already present.
///
/// # Errors
///
/// Returns an error when the URL is empty, not loopback, or already queued.
pub fn enqueue(db: &Database, id: &str, url: &str) -> Result<(), DownloadError> {
    if !loopback(url) {
        return Err(DownloadError::Host);
    }
    match enqueue_download(db, id, url) {
        Ok(()) => Ok(()),
        Err(StorageError::Duplicate) => Err(DownloadError::Duplicate),
        Err(error) => Err(error.into()),
    }
}

/// Pause, resume, or cancel.
///
/// # Errors
///
/// Returns an error when the row is missing.
pub fn set_status(db: &Database, id: &str, status: &str) -> Result<(), DownloadError> {
    set_download_status(db, id, status).map_err(DownloadError::from)
}

/// Run queued downloads into `dest` and upsert a library row for each file.
///
/// When `screen_open` is false, nothing is fetched. When `unmetered` is false, the call refuses.
///
/// # Errors
///
/// Returns an error when the network is metered or a fetch or write fails.
pub fn run<F>(
    db: &Database,
    dest: &Path,
    unmetered: bool,
    screen_open: bool,
    mut fetch: F,
) -> Result<(), DownloadError>
where
    F: FnMut(&str) -> Result<Vec<u8>, IoError>,
{
    if !screen_open {
        return Ok(());
    }
    if !unmetered {
        return Err(DownloadError::Metered);
    }
    fs::create_dir_all(dest)?;
    let rows = downloads(db)?;
    for row in rows {
        if row.status != "queued" {
            continue;
        }
        fetch_one(db, dest, &row, &mut fetch)?;
    }
    Ok(())
}

fn fetch_one<F>(
    db: &Database,
    dest: &Path,
    row: &DownloadRow,
    fetch: &mut F,
) -> Result<(), DownloadError>
where
    F: FnMut(&str) -> Result<Vec<u8>, IoError>,
{
    set_status(db, &row.id, "running")?;
    let bytes = fetch(&row.url)?;
    let path = dest.join(&row.id);
    fs::write(&path, &bytes)?;
    let file_type = file_extension(&row.url).unwrap_or("bin");
    db.upsert_item(CatalogItem {
        // clone: the catalog row owns its ids after the queue row is reused
        id: row.id.clone(),
        source: row.url.clone(),
        label: row.id.clone(),
        file_type: file_type.to_owned(),
        size: i64::try_from(bytes.len()).ok(),
        modified_ms: None,
        series_guess: String::new(),
        series: None,
        title: row.id.clone(),
        page_count: None,
        image_child_count: None,
        fingerprint: String::new(),
        enriched_fingerprint: None,
    })?;
    set_status(db, &row.id, "done")?;
    Ok(())
}

fn loopback(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
    else {
        return false;
    };
    let host = rest.split(['/', ':']).next().unwrap_or("");
    host == "127.0.0.1" || host == "localhost"
}

#[cfg(test)]
mod tests {
    use super::{enqueue, run, set_status};
    use folio_storage::{downloads, Database};
    use std::fs;

    #[test]
    fn download_queue_pauses_and_refuses_metered_networks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("folio.sqlite");
        let dest = dir.path().join("books");
        let db = Database::open(&path).expect("open");
        let url = "http://127.0.0.1:9/book.epub";
        enqueue(&db, "one", url).expect("enqueue");
        assert!(enqueue(&db, "two", url).is_err());
        set_status(&db, "one", "paused").expect("pause");
        set_status(&db, "one", "queued").expect("resume");
        set_status(&db, "one", "cancelled").expect("cancel");
        assert_eq!(downloads(&db).expect("rows")[0].status, "cancelled");

        enqueue(&db, "file", "http://127.0.0.1:9/story.epub").expect("second");
        run(&db, &dest, true, false, |_| Ok(b"nope".to_vec())).expect("screen closed");
        assert!(!dest.join("file").exists());
        assert!(run(&db, &dest, false, true, |_| Ok(b"nope".to_vec())).is_err());
        run(&db, &dest, true, true, |_| Ok(b"epub-bytes".to_vec())).expect("run");
        assert_eq!(fs::read(dest.join("file")).expect("bytes"), b"epub-bytes");
        let item = db
            .catalog()
            .expect("catalog")
            .into_iter()
            .find(|item| item.id == "file")
            .expect("row");
        assert_eq!(item.file_type, "epub");
        assert_eq!(
            downloads(&db)
                .expect("done")
                .into_iter()
                .find(|row| row.id == "file")
                .expect("row")
                .status,
            "done"
        );
    }
}
