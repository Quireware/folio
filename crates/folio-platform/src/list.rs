//! Listing metadata. Pass 1 does not open publication bytes.

/// One path from a directory walk or a document cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedPath {
    /// Path relative to the library root, using `/`.
    pub relative: String,
    /// Byte length. `None` means the provider did not supply one.
    pub size: Option<u64>,
    /// Modification time in Unix milliseconds. `None` means it was not supplied.
    pub modified_ms: Option<i64>,
    /// Provider MIME type, when the cursor has one.
    pub mime: Option<String>,
    /// Whether this entry is a directory.
    pub is_dir: bool,
}

/// A cursor listing and whether it is safe to retire missing rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorListing {
    /// Documents the cursor returned.
    pub entries: Vec<ListedPath>,
    /// `false` when the cursor was still loading or the walk was cancelled.
    pub complete: bool,
}
