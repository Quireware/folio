//! Cover files for the shelf. One thumbnail is written beside the database.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use folio_formats::{
    file_extension, open_cb7, open_cbr, open_cbt, open_cbz, open_epub, render_pdf_png_scaled,
};
use folio_library::cover_cache_path;

use crate::session::{book_path, page_data_url, FileSource, SessionError};

/// Read the cached thumbnail, or extract one and store it.
///
/// `None` means this book has no cover. The empty file is how the next launch
/// knows not to open it again.
///
/// # Errors
///
/// Returns an error when the book cannot be read or the thumbnail cannot be stored.
/// A parser rejection is returned so the caller can retry on a later launch.
pub(crate) fn materialise(
    dir: &Path,
    id: &str,
    library: &Path,
    kind: &str,
) -> Result<Option<String>, SessionError> {
    let cached = cover_cache_path(dir, id);
    if cached.is_file() {
        return data_url(&cached);
    }
    let path = book_path(library, id)?;
    let extracted = extract_cover(&path, kind)?;
    let stored = extracted
        .as_deref()
        .and_then(thumbnail_jpeg)
        .unwrap_or_default();
    write_atomic(&cached, &stored)?;
    if stored.is_empty() {
        Ok(None)
    } else {
        Ok(Some(page_data_url("cover.jpg", &stored)))
    }
}

fn data_url(path: &Path) -> Result<Option<String>, SessionError> {
    let bytes = fs::read(path)?;
    if bytes.is_empty() {
        Ok(None)
    } else {
        Ok(Some(page_data_url("cover.jpg", &bytes)))
    }
}

fn extract_cover(path: &Path, kind: &str) -> Result<Option<Vec<u8>>, SessionError> {
    match kind {
        "epub" => {
            let mut source = FileSource::open(path)?;
            Ok(open_epub(&mut source)?.cover)
        }
        "pdf" => match render_pdf_png_scaled(&fs::read(path)?, 0, 0.5) {
            Ok(png) => Ok(Some(png)),
            Err(error) => Err(error.into()),
        },
        "cbz" | "cb7" | "cbt" | "cbr" => comic_cover(path, kind),
        "image-folder" => folder_cover(path),
        _ => Ok(None),
    }
}

fn comic_cover(path: &Path, kind: &str) -> Result<Option<Vec<u8>>, SessionError> {
    let mut source = FileSource::open(path)?;
    let comic = match kind {
        "cbz" => open_cbz(&mut source, None)?,
        "cb7" => open_cb7(&mut source, None)?,
        "cbt" => open_cbt(&mut source, None)?,
        "cbr" => open_cbr(&mut source, None)?,
        _ => return Ok(None),
    };
    Ok(comic.cover)
}

fn folder_cover(path: &Path) -> Result<Option<Vec<u8>>, SessionError> {
    let mut names = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_symlink() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if is_image_name(&name) {
            names.push(name);
        }
    }
    names.sort();
    let Some(name) = names.first() else {
        return Ok(None);
    };
    Ok(Some(fs::read(path.join(name))?))
}

fn is_image_name(name: &str) -> bool {
    let Some(extension) = file_extension(name) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "avif"
    )
}

fn thumbnail_jpeg(bytes: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory(bytes).ok()?;
    let fitted = image.thumbnail(360, 540);
    let rgb = image::DynamicImage::ImageRgb8(fitted.to_rgb8());
    let mut encoded = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 70);
    rgb.write_with_encoder(encoder).ok()?;
    if encoded.is_empty() {
        None
    } else {
        Some(encoded)
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), SessionError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = temporary_path(path);
    fs::write(&temporary, bytes)?;
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            if let Err(remove_error) = fs::remove_file(&temporary) {
                if remove_error.kind() != ErrorKind::NotFound {
                    tracing::debug!(%remove_error, "cover temporary file stayed behind");
                }
            }
            Err(error.into())
        }
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::thumbnail_jpeg;

    #[test]
    fn thumbnail_jpeg_fits_a_flat_png() {
        let mut image = image::RgbImage::new(800, 1200);
        for pixel in image.pixels_mut() {
            *pixel = image::Rgb([20, 40, 60]);
        }
        let dynamic = image::DynamicImage::ImageRgb8(image);
        let mut png = std::io::Cursor::new(Vec::new());
        dynamic
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("png");
        let thumb = thumbnail_jpeg(png.get_ref()).expect("jpeg");
        assert!(thumb.starts_with(&[0xFF, 0xD8]));
        let decoded = image::load_from_memory(&thumb).expect("decoded");
        assert!(decoded.width() <= 360);
        assert!(decoded.height() <= 540);
        assert!(thumbnail_jpeg(b"not an image").is_none());
    }

    #[test]
    fn materialise_stores_a_folder_cover_and_an_empty_file_when_there_is_none() {
        let root = tempfile::tempdir().expect("dir");
        let library = root.path().join("library");
        let book = library.join("Scans");
        std::fs::create_dir_all(&book).expect("book");
        let mut image = image::RgbImage::new(20, 30);
        for pixel in image.pixels_mut() {
            *pixel = image::Rgb([9, 8, 7]);
        }
        image::DynamicImage::ImageRgb8(image)
            .save(book.join("page.png"))
            .expect("png");
        let covers = root.path().join("covers");
        let url = super::materialise(&covers, "Scans", &library, "image-folder")
            .expect("cover")
            .expect("url");
        assert!(url.starts_with("data:image/jpeg;base64,"));
        let again = super::materialise(&covers, "Scans", &library, "image-folder")
            .expect("cached")
            .expect("url");
        assert_eq!(url, again);
        std::fs::write(library.join("note.txt"), b"x").expect("note");
        let none = super::materialise(&covers, "note.txt", &library, "mobi").expect("stored");
        assert!(none.is_none());
        let none_again = super::materialise(&covers, "note.txt", &library, "mobi").expect("cached");
        assert!(none_again.is_none());
    }
}
