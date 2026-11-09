//! Desktop paths, document listings, grants, and staging for streams that cannot seek.
#![deny(missing_docs)]

mod android;
mod desktop;
mod grant;
mod list;
mod secret;
mod stage;

pub use android::{
    binding, list_document_cursor, resolve_theme, shortcuts, AndroidError, BackgroundJob,
    CursorDocument, DocumentCursor, ExternalSdRoot, KeyLayout, ReaderCommand, ResolvedTheme,
    ServiceGate, SystemPalette, WidgetSnapshot,
};
pub use desktop::list_directory;
pub use grant::{GrantStore, TreeGrant};
pub use list::{CursorListing, ListedPath};
pub use secret::{SecretError, SecretStore};
pub use stage::{stage_unseekable, PlatformError, StagedSource};

use std::path::{Path, PathBuf};

/// Directory name folio uses under an application-data root.
pub fn app_directory_name() -> &'static str {
    "folio"
}

/// `root` joined with [`app_directory_name`].
pub fn data_dir(root: &Path) -> PathBuf {
    root.join(app_directory_name())
}
