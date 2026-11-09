//! Library naming and the two-pass catalog.
#![deny(missing_docs)]

mod collection;
mod download;
mod kavita;
mod scan;

pub use collection::{history_ids, smart_match, smart_members, SmartItem, SmartRule};
pub use download::{enqueue, run, set_status, DownloadError};
pub use kavita::{leave_foreground, recover, run as run_kavita, same_origin, KavitaDownloadError};

pub use scan::{
    cover_cache_path, identify, open_listed, scan_directory, scan_listing, IdentityRow,
    ParsedPublication, ScanDepth, ScanError, ScanOutcome, ScanRequest,
};

/// Display label for a file name: the last path component without its extension.
///
/// A name with no extension is returned unchanged. A leading-dot name is kept whole.
pub fn filename_label(file_name: &str) -> &str {
    let base = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    match base.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && !extension.is_empty() => stem,
        _ => base,
    }
}
