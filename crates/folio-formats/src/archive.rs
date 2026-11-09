//! CBZ, CB7, and CBT read into memory. Nothing is extracted to a directory.

use std::io::{self, Read};

use folio_core::SeekableSource;
use rar_codec::{Archive, EntryKind};
use sevenz_rust2::{ArchiveReader, Password};
use zip::ZipArchive;

use crate::error::FormatError;
use crate::names::{base_name, entry_escapes, is_page_image, natural_cmp};
use crate::reader::SourceReader;
use crate::sidecar::comic_info_series;
use crate::{Comic, Page};

/// One zip entry already copied into memory.
pub(crate) struct ZipEntry {
    pub(crate) name: String,
    pub(crate) bytes: Vec<u8>,
}

/// Read every zip entry. An escaping name fails the whole archive.
pub(crate) fn zip_files(source: &mut dyn SeekableSource) -> Result<Vec<ZipEntry>, FormatError> {
    let reader = SourceReader::new(source)?;
    let mut archive = ZipArchive::new(reader).map_err(zip_error)?;
    let mut entries = Vec::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(zip_error)?;
        if file.enclosed_name().is_none() {
            return Err(FormatError::PathEscape);
        }
        let name = normalize_name(file.name());
        if entry_escapes(&name) {
            return Err(FormatError::PathEscape);
        }
        if name.ends_with('/') {
            continue;
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        entries.push(ZipEntry { name, bytes });
    }
    Ok(entries)
}

/// Open a CBZ.
///
/// `sidecar_cover`, when present, replaces the first page as the cover.
/// Page bytes stay the archive images. ComicInfo `<Series>` is returned when
/// that file is in the archive.
///
/// # Errors
///
/// Returns [`FormatError::PathEscape`] for an absolute entry or one that
/// contains `..`, and [`FormatError::Invalid`] when the zip cannot be read.
pub fn open_cbz(
    source: &mut dyn SeekableSource,
    sidecar_cover: Option<&[u8]>,
) -> Result<Comic, FormatError> {
    let entries = zip_files(source)?;
    let named = entries
        .into_iter()
        .map(|entry| (entry.name, entry.bytes))
        .collect();
    Ok(assemble(named, sidecar_cover))
}

/// Open a CBR. The archive is decoded in memory and never written out.
///
/// RAR is read by `rar-codec` (Apache-2.0). This build does not link UnRAR.
///
/// # Errors
///
/// Returns [`FormatError::PathEscape`] or [`FormatError::Invalid`], as [`open_cbz`] does.
pub fn open_cbr(
    source: &mut dyn SeekableSource,
    sidecar_cover: Option<&[u8]>,
) -> Result<Comic, FormatError> {
    let bytes = read_source(source, "cbr")?;
    let archive = Archive::open(&bytes).map_err(|error| {
        tracing::debug!(%error, "rar rejected");
        FormatError::Invalid { kind: "cbr" }
    })?;
    let mut found = Vec::new();
    for entry in archive.entries() {
        if entry.kind != EntryKind::File {
            continue;
        }
        if entry.encrypted {
            return Err(FormatError::Drm);
        }
        let name = normalize_name(entry.name.as_ref());
        if entry_escapes(&name) {
            return Err(FormatError::PathEscape);
        }
        let data = archive.extract(entry).map_err(|error| {
            tracing::debug!(%error, "rar entry rejected");
            FormatError::Invalid { kind: "cbr" }
        })?;
        found.push((name, data));
    }
    Ok(assemble(found, sidecar_cover))
}

/// Open a CB7. The archive is decoded in memory and never written out.
///
/// # Errors
///
/// Returns [`FormatError::PathEscape`] or [`FormatError::Invalid`], as [`open_cbz`] does.
pub fn open_cb7(
    source: &mut dyn SeekableSource,
    sidecar_cover: Option<&[u8]>,
) -> Result<Comic, FormatError> {
    let reader = SourceReader::new(source)?;
    let mut archive = ArchiveReader::new(reader, Password::empty()).map_err(sevenz_error)?;
    let mut found = Vec::new();
    let mut failure = None;
    archive
        .for_each_entries(|entry, data| {
            let mut bytes = Vec::new();
            match data.read_to_end(&mut bytes) {
                Ok(_) => {
                    if failure.is_none() && !entry.is_directory() {
                        let name = normalize_name(entry.name());
                        if entry_escapes(&name) {
                            failure = Some(FormatError::PathEscape);
                        } else {
                            // to_owned: the 7z entry name is only borrowed for this callback
                            found.push((name, bytes));
                        }
                    }
                    Ok(true)
                }
                Err(error) => {
                    failure = Some(FormatError::Io(error));
                    Ok(false)
                }
            }
        })
        .map_err(sevenz_error)?;
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(assemble(found, sidecar_cover))
}

/// Open a CBT (tar). The archive is not extracted.
///
/// # Errors
///
/// Returns [`FormatError::PathEscape`] or [`FormatError::Invalid`], as [`open_cbz`] does.
pub fn open_cbt(
    source: &mut dyn SeekableSource,
    sidecar_cover: Option<&[u8]>,
) -> Result<Comic, FormatError> {
    let reader = SourceReader::new(source)?;
    let mut archive = tar::Archive::new(reader);
    let mut found = Vec::new();
    let entries = archive.entries().map_err(tar_error)?;
    for entry in entries {
        let mut entry = entry.map_err(tar_error)?;
        let name = tar_name(&entry)?;
        if entry_escapes(&name) || name.split(['/', '\\']).any(|part| part == "..") {
            return Err(FormatError::PathEscape);
        }
        if name.ends_with('/') {
            continue;
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        found.push((name, bytes));
    }
    Ok(assemble(found, sidecar_cover))
}

fn assemble(entries: Vec<(String, Vec<u8>)>, sidecar_cover: Option<&[u8]>) -> Comic {
    let mut series = None;
    let mut pages = Vec::new();
    for (name, bytes) in entries {
        if base_name(&name).eq_ignore_ascii_case("comicinfo.xml") {
            if series.is_none() {
                let text = String::from_utf8_lossy(&bytes);
                let text = text.trim_start_matches('\u{feff}');
                series = comic_info_series(text);
            }
            continue;
        }
        if is_page_image(&name) {
            pages.push(Page { name, bytes });
        }
    }
    pages.sort_by(|left, right| natural_cmp(&left.name, &right.name));
    let cover = match sidecar_cover {
        // to_owned: the cover outlives the caller's slice
        Some(bytes) => Some(bytes.to_vec()),
        // clone: the cover is stored apart from the page that supplied it
        None => pages.first().map(|page| page.bytes.clone()),
    };
    Comic {
        series,
        pages,
        cover,
    }
}

fn read_source(
    source: &mut dyn SeekableSource,
    kind: &'static str,
) -> Result<Vec<u8>, FormatError> {
    let len = usize::try_from(source.len()?).map_err(|_| FormatError::Invalid { kind })?;
    let mut bytes = vec![0u8; len];
    let read = source.read_at(0, &mut bytes)?;
    if read != bytes.len() {
        return Err(FormatError::Invalid { kind });
    }
    Ok(bytes)
}

fn normalize_name(name: &str) -> String {
    let mut trimmed = name;
    while let Some(rest) = trimmed.strip_prefix("./") {
        trimmed = rest;
    }
    trimmed.replace('\\', "/")
}

fn tar_name(entry: &tar::Entry<'_, SourceReader<'_>>) -> Result<String, FormatError> {
    let raw = match entry.path() {
        Ok(path) => path,
        Err(error) => {
            tracing::debug!(%error, "tar entry name rejected");
            return Err(FormatError::PathEscape);
        }
    };
    let Some(name) = raw.to_str() else {
        return Err(FormatError::Invalid { kind: "cbt" });
    };
    Ok(normalize_name(name))
}

fn zip_error(error: zip::result::ZipError) -> FormatError {
    match error {
        zip::result::ZipError::Io(error) => FormatError::Io(error),
        other => {
            tracing::debug!(error = %other, "zip rejected");
            FormatError::Invalid { kind: "zip" }
        }
    }
}

fn sevenz_error(error: sevenz_rust2::Error) -> FormatError {
    tracing::debug!(%error, "cb7 rejected");
    FormatError::Invalid { kind: "cb7" }
}

fn tar_error(error: io::Error) -> FormatError {
    tracing::debug!(%error, "cbt rejected");
    FormatError::Io(error)
}
