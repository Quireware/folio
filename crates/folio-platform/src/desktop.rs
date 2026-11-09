//! Desktop directory listing. File bytes are not read.

use std::fs;
use std::path::{Component, Path};
use std::time::UNIX_EPOCH;

use crate::list::ListedPath;
use crate::stage::PlatformError;

/// Walk `root` and return listing metadata for every file and directory under it.
///
/// Symlinks are skipped. The root itself is not an entry.
///
/// # Errors
///
/// Returns an error when a directory in the walk cannot be read.
pub fn list_directory(root: &Path) -> Result<Vec<ListedPath>, PlatformError> {
    let mut out = Vec::new();
    walk(root, root, &mut out)?;
    Ok(out)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<ListedPath>) -> Result<(), PlatformError> {
    let entries = fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        let Some(relative) = relative_to(root, &path) else {
            continue;
        };
        let meta = entry.metadata()?;
        out.push(ListedPath {
            relative,
            size: Some(meta.len()),
            modified_ms: modified_ms(&meta),
            mime: None,
            is_dir: file_type.is_dir(),
        });
        if file_type.is_dir() {
            walk(root, &path, out)?;
        }
    }
    Ok(())
}

fn relative_to(root: &Path, path: &Path) -> Option<String> {
    let stripped = path.strip_prefix(root).ok()?;
    let mut relative = String::new();
    for (index, component) in stripped.components().enumerate() {
        let Component::Normal(part) = component else {
            return None;
        };
        let part = part.to_str()?;
        if part.is_empty() || part.contains('\0') {
            return None;
        }
        if index > 0 {
            relative.push('/');
        }
        relative.push_str(part);
    }
    if relative.is_empty() {
        None
    } else {
        Some(relative)
    }
}

fn modified_ms(meta: &fs::Metadata) -> Option<i64> {
    let modified = meta.modified().ok()?;
    let elapsed = modified.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(elapsed.as_millis()).ok()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::list_directory;

    #[test]
    fn directory_listing_does_not_need_file_contents() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("folio-list-{nanos}"));
        fs::create_dir(&root).expect("root");
        fs::write(root.join("note.txt"), b"hello").expect("file");
        let listed = list_directory(&root).expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].relative, "note.txt");
        assert_eq!(listed[0].size, Some(5));
        assert!(listed[0].modified_ms.is_some());
        fs::remove_dir_all(&root).expect("cleanup");
    }
}
