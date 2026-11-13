//! Kavita chapter downloads. They do not join the local shelf.
//!
//! A partial file is deleted. The next run starts at the first byte.

use std::fs;
use std::io::Error as IoError;
use std::path::Path;

use folio_storage::{
    claim_kavita_download, kavita_download_status, kavita_downloads, settle_kavita_download,
    Database, KavitaDownloadRow, StorageError,
};
use thiserror::Error;
use url::Url;

/// Why a Kavita download did not finish.
#[derive(Debug, Error)]
pub enum KavitaDownloadError {
    /// Wi-Fi-only is set and the network is metered.
    #[error("wifi-only refused a metered network")]
    Metered,
    /// The URL is not on the saved Kavita origin.
    #[error("download url is not on the saved server")]
    Origin,
    /// The chapter is missing a page or an image.
    #[error("the download is missing a page")]
    Incomplete,
    /// Leaving the screen or removing the server stopped the download while it ran.
    #[error("the download was stopped")]
    Stopped,
    /// Storage failed.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// The file could not be written.
    #[error(transparent)]
    Io(#[from] IoError),
}

/// A running row left by a killed process goes back to `queued`. Its partial is deleted.
///
/// # Errors
///
/// Returns an error when the queue cannot be read or a file cannot be removed.
pub fn recover(db: &Database, root: &Path) -> Result<(), KavitaDownloadError> {
    reset_running(db, root)
}

/// Leaving the screen cancels the current file and queues it from the first byte.
///
/// # Errors
///
/// Returns an error when the queue cannot be read or a file cannot be removed.
pub fn leave_foreground(db: &Database, root: &Path) -> Result<(), KavitaDownloadError> {
    reset_running(db, root)
}

/// Run queued downloads while `foreground` is true and the network is unmetered.
///
/// `fetch` writes the chapter into the directory it is given and returns whether every page
/// and that page's images were stored. A `false` return deletes the directory and queues the
/// row again. The catalog is not updated.
///
/// # Errors
///
/// Returns an error when the network is metered, the URL origin does not match, or a fetch fails.
pub fn run<F>(
    db: &Database,
    root: &Path,
    saved_origin: &str,
    unmetered: bool,
    foreground: bool,
    mut fetch: F,
) -> Result<(), KavitaDownloadError>
where
    F: FnMut(&KavitaDownloadRow, &Path) -> Result<bool, IoError>,
{
    if !foreground {
        return leave_foreground(db, root);
    }
    if !unmetered {
        return Err(KavitaDownloadError::Metered);
    }
    fs::create_dir_all(root)?;
    let rows = kavita_downloads(db)?;
    for row in rows {
        if row.status != "queued" {
            continue;
        }
        if !same_origin(saved_origin, &row.url) {
            return Err(KavitaDownloadError::Origin);
        }
        if !claim_kavita_download(db, &row.id)? {
            continue;
        }
        let directory = root.join(&row.id);
        fs::create_dir_all(&directory)?;
        abandon_if_gone(db, &row.id, &directory)?;
        let fetched = fetch(&row, &directory);
        let finished = matches!(fetched, Ok(true));
        if !finished {
            // The row is still `running`, so no other loop can claim it while this one tidies.
            remove_partial(&directory)?;
        }
        let status = if finished { "done" } else { "queued" };
        if !settle_kavita_download(db, &row.id, status)? {
            abandon_if_gone(db, &row.id, &directory)?;
            return Err(KavitaDownloadError::Stopped);
        }
        match fetched {
            Ok(true) => {}
            Ok(false) => return Err(KavitaDownloadError::Incomplete),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn abandon_if_gone(db: &Database, id: &str, directory: &Path) -> Result<(), KavitaDownloadError> {
    if kavita_download_status(db, id)?.is_none() {
        remove_partial(directory)?;
        return Err(KavitaDownloadError::Stopped);
    }
    Ok(())
}

fn remove_partial(directory: &Path) -> Result<(), KavitaDownloadError> {
    match fs::remove_dir_all(directory) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn reset_running(db: &Database, root: &Path) -> Result<(), KavitaDownloadError> {
    let rows = kavita_downloads(db)?;
    for row in rows {
        if row.status != "running" {
            continue;
        }
        // Settling first tells the loop that runs this row to stop instead of finishing it.
        if settle_kavita_download(db, &row.id, "queued")? {
            remove_partial(&root.join(&row.id))?;
        }
    }
    Ok(())
}

/// `target` is on the same scheme, host, and port as `saved`.
pub fn same_origin(saved: &str, target: &str) -> bool {
    let Ok(saved) = Url::parse(saved) else {
        return false;
    };
    let Ok(target) = Url::parse(target) else {
        return false;
    };
    matches!(saved.scheme(), "http" | "https")
        && saved.scheme() == target.scheme()
        && saved.host_str().is_some()
        && saved.host_str() == target.host_str()
        && saved.port_or_known_default() == target.port_or_known_default()
}

#[cfg(test)]
mod tests {
    use super::{leave_foreground, recover, run, same_origin, KavitaDownloadError};
    use folio_storage::{
        claim_kavita_download, delete_kavita_download, enqueue_kavita_download,
        kavita_download_status, Database,
    };
    use std::fs;
    use std::io::Error as IoError;

    fn test_db() -> Database {
        let path = std::env::temp_dir().join(format!(
            "folio-kavita-dl-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        if path.exists() {
            fs::remove_file(&path).expect("remove");
        }
        Database::open(&path).expect("open")
    }

    #[test]
    fn metered_refuses_and_a_foreign_origin_is_rejected() {
        let db = test_db();
        let root = tempfile::tempdir().expect("root");
        enqueue_kavita_download(&db, "ch-1", "server-1", "http://127.0.0.1:9/page", 1)
            .expect("queue");
        let error = run(
            &db,
            root.path(),
            "http://127.0.0.1:9",
            false,
            true,
            |_row, _dir| Ok(true),
        )
        .expect_err("metered");
        assert!(matches!(error, KavitaDownloadError::Metered));
        assert_eq!(
            kavita_download_status(&db, "ch-1")
                .expect("status")
                .as_deref(),
            Some("queued")
        );
        let error = run(
            &db,
            root.path(),
            "http://192.168.1.20:5000",
            true,
            true,
            |_row, _dir| Ok(true),
        )
        .expect_err("origin");
        assert!(matches!(error, KavitaDownloadError::Origin));
        assert!(same_origin("http://127.0.0.1:9", "http://127.0.0.1:9/page"));
    }

    #[test]
    fn leaving_foreground_deletes_the_partial_and_queues_from_the_start() {
        let db = test_db();
        let root = tempfile::tempdir().expect("root");
        enqueue_kavita_download(&db, "ch-1", "server-1", "http://127.0.0.1:9/page", 1)
            .expect("queue");
        assert!(claim_kavita_download(&db, "ch-1").expect("running"));
        let partial = root.path().join("ch-1");
        fs::create_dir_all(&partial).expect("dir");
        fs::write(partial.join("page-0"), b"partial").expect("bytes");
        leave_foreground(&db, root.path()).expect("leave");
        assert!(!partial.exists());
        assert_eq!(
            kavita_download_status(&db, "ch-1")
                .expect("status")
                .as_deref(),
            Some("queued")
        );
        recover(&db, root.path()).expect("recover");
    }

    #[test]
    fn a_complete_download_does_not_write_a_catalog_row() {
        let db = test_db();
        let root = tempfile::tempdir().expect("root");
        enqueue_kavita_download(&db, "ch-1", "server-1", "http://127.0.0.1:9/page", 1)
            .expect("queue");
        run(
            &db,
            root.path(),
            "http://127.0.0.1:9",
            true,
            true,
            |_row, dir| {
                fs::write(dir.join("page-0"), b"page")?;
                Ok(true)
            },
        )
        .expect("run");
        assert_eq!(
            kavita_download_status(&db, "ch-1")
                .expect("status")
                .as_deref(),
            Some("done")
        );
        assert!(db.catalog().expect("catalog").is_empty());
        enqueue_kavita_download(&db, "ch-2", "server-1", "http://127.0.0.1:9/other", 2)
            .expect("queue 2");
        let missing = run(
            &db,
            root.path(),
            "http://127.0.0.1:9",
            true,
            true,
            |_row, _dir| Ok::<bool, IoError>(false),
        );
        assert!(missing.is_err());
        assert_eq!(
            kavita_download_status(&db, "ch-2")
                .expect("status")
                .as_deref(),
            Some("queued")
        );
    }

    #[test]
    fn leaving_the_screen_during_a_fetch_stops_the_loop() {
        let db = test_db();
        let root = tempfile::tempdir().expect("root");
        enqueue_kavita_download(&db, "ch-1", "server-1", "http://127.0.0.1:9/page", 1)
            .expect("queue");
        enqueue_kavita_download(&db, "ch-2", "server-1", "http://127.0.0.1:9/next", 2)
            .expect("queue 2");
        let mut fetched = Vec::new();
        let stopped = run(
            &db,
            root.path(),
            "http://127.0.0.1:9",
            true,
            true,
            |row, dir| {
                fetched.push(row.id.clone());
                fs::write(dir.join("page-0"), b"partial")?;
                leave_foreground(&db, root.path()).map_err(IoError::other)?;
                Ok(true)
            },
        )
        .expect_err("stopped");
        assert!(matches!(stopped, KavitaDownloadError::Stopped));
        assert_eq!(fetched, ["ch-1"]);
        assert!(!root.path().join("ch-1").exists());
        for id in ["ch-1", "ch-2"] {
            assert_eq!(
                kavita_download_status(&db, id).expect("status").as_deref(),
                Some("queued")
            );
        }
    }

    #[test]
    fn a_fetch_that_loses_its_row_deletes_the_directory() {
        let db = test_db();
        let root = tempfile::tempdir().expect("root");
        enqueue_kavita_download(&db, "ch-1", "server-1", "http://127.0.0.1:9/page", 1)
            .expect("queue");
        let stopped = run(
            &db,
            root.path(),
            "http://127.0.0.1:9",
            true,
            true,
            |row, dir| {
                fs::write(dir.join("page-0"), b"partial")?;
                delete_kavita_download(&db, &row.id).map_err(IoError::other)?;
                Ok(true)
            },
        )
        .expect_err("stopped");
        assert!(matches!(stopped, KavitaDownloadError::Stopped));
        assert!(!root.path().join("ch-1").exists());
        assert!(kavita_download_status(&db, "ch-1")
            .expect("status")
            .is_none());
    }
}
