//! PalmDOC and MOBI text. Huffman and Kindle KF8 compression are rejected.

use folio_core::SeekableSource;

use crate::error::FormatError;

/// Decoded MOBI text. Markup is left for the reader to turn into blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobiBook {
    /// Full name from the header, or empty when the file has none.
    pub title: String,
    /// Book text, UTF-8. HTML is not stripped here.
    pub text: String,
}

/// Open a MOBI, AZW, or PRC whose text is uncompressed or PalmDOC-compressed.
///
/// # Errors
///
/// Returns [`FormatError::Drm`] when the record is encrypted, and
/// [`FormatError::Invalid`] when the header or the compression is not readable.
pub fn open_mobi(source: &mut dyn SeekableSource) -> Result<MobiBook, FormatError> {
    let bytes = read_source(source)?;
    parse_mobi(&bytes)
}

fn parse_mobi(bytes: &[u8]) -> Result<MobiBook, FormatError> {
    if bytes.len() < 78 || (&bytes[60..64] != b"BOOK" && &bytes[60..64] != b"TEXt") {
        return Err(invalid());
    }
    if &bytes[64..68] != b"MOBI" {
        return Err(invalid());
    }
    let records = usize::from(read_u16(bytes, 76)?);
    if records == 0 {
        return Err(invalid());
    }
    let mut offsets = Vec::with_capacity(records);
    let mut cursor = 78;
    for _ in 0..records {
        let offset = usize::try_from(read_u32(bytes, cursor)?).map_err(|_| invalid())?;
        if offset > bytes.len() || offsets.last().is_some_and(|previous| offset < *previous) {
            return Err(invalid());
        }
        offsets.push(offset);
        cursor = cursor.checked_add(8).ok_or_else(invalid)?;
    }
    let record0 = record_bytes(bytes, &offsets, 0)?;
    if record0.len() < 16 {
        return Err(invalid());
    }
    let compression = read_u16(record0, 0)?;
    let text_length = usize::try_from(read_u32(record0, 4)?).map_err(|_| invalid())?;
    let text_records = usize::from(read_u16(record0, 8)?);
    let record_size = usize::from(read_u16(record0, 10)?);
    let encryption = read_u16(record0, 12)?;
    if encryption != 0 {
        return Err(FormatError::Drm);
    }
    if text_length > MAX_TEXT || text_records > records {
        return Err(invalid());
    }
    let title = mobi_title(record0);
    let mut raw = Vec::new();
    let chunk_limit = if record_size == 0 { 4096 } else { record_size };
    for index in 1..=text_records {
        if raw.len() >= text_length {
            break;
        }
        let record = record_bytes(bytes, &offsets, index)?;
        let room = text_length - raw.len();
        let limit = chunk_limit.min(room);
        let decoded = match compression {
            1 => {
                let take = record.len().min(room);
                record[..take].to_vec()
            }
            2 => palmdoc(record, limit)?,
            _ => return Err(invalid()),
        };
        raw.extend_from_slice(&decoded);
    }
    raw.truncate(text_length);
    let encoding = text_encoding(record0);
    Ok(MobiBook {
        title,
        text: decode_text(&raw, encoding),
    })
}

fn mobi_title(record0: &[u8]) -> String {
    if record0.len() < 16 + 92 || &record0[16..20] != b"MOBI" {
        return String::new();
    }
    let Ok(header_length) =
        read_u32(record0, 20).and_then(|value| usize::try_from(value).map_err(|_| invalid()))
    else {
        return String::new();
    };
    if header_length < 92 {
        return String::new();
    }
    let Ok(offset) =
        read_u32(record0, 16 + 84).and_then(|value| usize::try_from(value).map_err(|_| invalid()))
    else {
        return String::new();
    };
    let Ok(length) =
        read_u32(record0, 16 + 88).and_then(|value| usize::try_from(value).map_err(|_| invalid()))
    else {
        return String::new();
    };
    let Some(end) = offset.checked_add(length) else {
        return String::new();
    };
    let Some(bytes) = record0.get(offset..end) else {
        return String::new();
    };
    let encoding = text_encoding(record0);
    let mut title = decode_text(bytes, encoding);
    while title.ends_with('\0') {
        title.pop();
    }
    title
}

fn text_encoding(record0: &[u8]) -> u32 {
    if record0.len() >= 16 + 16 && &record0[16..20] == b"MOBI" {
        read_u32(record0, 16 + 12).unwrap_or(65001)
    } else {
        65001
    }
}

fn palmdoc(input: &[u8], limit: usize) -> Result<Vec<u8>, FormatError> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < input.len() && out.len() < limit {
        let byte = input[index];
        index += 1;
        if (1..=8).contains(&byte) {
            let count = usize::from(byte);
            let end = index.checked_add(count).ok_or_else(invalid)?;
            let literal = input.get(index..end).ok_or_else(invalid)?;
            let take = literal.len().min(limit - out.len());
            out.extend_from_slice(&literal[..take]);
            index = end;
        } else if byte < 128 {
            out.push(byte);
        } else if byte >= 192 {
            if out.len().saturating_add(1) >= limit {
                break;
            }
            out.push(b' ');
            if out.len() < limit {
                out.push(byte ^ 0x80);
            }
        } else {
            let next = *input.get(index).ok_or_else(invalid)?;
            index += 1;
            let pair = (u16::from(byte) << 8) | u16::from(next);
            let distance = usize::from((pair >> 3) & 0x7FF);
            let length = usize::from((pair & 7) + 3);
            if distance == 0 || distance > out.len() {
                return Err(invalid());
            }
            for _ in 0..length {
                if out.len() >= limit {
                    break;
                }
                let source = out.len() - distance;
                let copied = out[source];
                out.push(copied);
            }
        }
    }
    Ok(out)
}

fn decode_text(bytes: &[u8], encoding: u32) -> String {
    if encoding == 65001 {
        if let Ok(text) = std::str::from_utf8(bytes) {
            return text.to_owned();
        }
    }
    bytes.iter().copied().map(char::from).collect()
}

fn record_bytes<'a>(
    bytes: &'a [u8],
    offsets: &[usize],
    index: usize,
) -> Result<&'a [u8], FormatError> {
    let start = *offsets.get(index).ok_or_else(invalid)?;
    let end = offsets.get(index + 1).copied().unwrap_or(bytes.len());
    if start > end || end > bytes.len() {
        return Err(invalid());
    }
    Ok(&bytes[start..end])
}

fn read_u16(bytes: &[u8], at: usize) -> Result<u16, FormatError> {
    let end = at.checked_add(2).ok_or_else(invalid)?;
    let slice = bytes.get(at..end).ok_or_else(invalid)?;
    let mut array = [0u8; 2];
    array.copy_from_slice(slice);
    Ok(u16::from_be_bytes(array))
}

fn read_u32(bytes: &[u8], at: usize) -> Result<u32, FormatError> {
    let end = at.checked_add(4).ok_or_else(invalid)?;
    let slice = bytes.get(at..end).ok_or_else(invalid)?;
    let mut array = [0u8; 4];
    array.copy_from_slice(slice);
    Ok(u32::from_be_bytes(array))
}

fn read_source(source: &mut dyn SeekableSource) -> Result<Vec<u8>, FormatError> {
    let len = usize::try_from(source.len()?).map_err(|_| invalid())?;
    let mut bytes = vec![0u8; len];
    let read = source.read_at(0, &mut bytes)?;
    if read != bytes.len() {
        return Err(invalid());
    }
    Ok(bytes)
}

fn invalid() -> FormatError {
    FormatError::Invalid { kind: "mobi" }
}

const MAX_TEXT: usize = 32 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::{open_mobi, palmdoc};
    use folio_core::SliceSource;

    #[test]
    fn palmdoc_copies_a_back_reference() {
        let mut compressed = b"Hello ".to_vec();
        compressed.extend_from_slice(&[0x80, 0x32]);
        let decoded = palmdoc(&compressed, 64).expect("decode");
        assert_eq!(decoded, b"Hello Hello");
    }

    #[test]
    fn uncompressed_mobi_keeps_multibyte_text() {
        let bytes = sample_mobi("Cafe", "Hello café");
        let mut source = SliceSource::new(&bytes);
        let book = open_mobi(&mut source).expect("mobi");
        assert_eq!(book.title, "Cafe");
        assert_eq!(book.text, "Hello café");
    }

    #[test]
    fn huffman_mobi_is_rejected() {
        let mut bytes = sample_mobi("Cafe", "Hello");
        let offset_bytes: [u8; 4] = bytes[78..82].try_into().expect("offset");
        let record0 = usize::try_from(u32::from_be_bytes(offset_bytes)).expect("record");
        bytes[record0] = 0x44;
        bytes[record0 + 1] = 0x48;
        let mut source = SliceSource::new(&bytes);
        let error = open_mobi(&mut source).expect_err("huffman");
        assert!(matches!(
            error,
            crate::FormatError::Invalid { kind: "mobi" }
        ));
    }

    #[test]
    fn encrypted_mobi_is_drm() {
        let mut bytes = sample_mobi("Cafe", "Hello");
        let offset_bytes: [u8; 4] = bytes[78..82].try_into().expect("offset");
        let record0 = usize::try_from(u32::from_be_bytes(offset_bytes)).expect("record");
        bytes[record0 + 12] = 0;
        bytes[record0 + 13] = 1;
        let mut source = SliceSource::new(&bytes);
        let error = open_mobi(&mut source).expect_err("encrypted");
        assert!(matches!(error, crate::FormatError::Drm));
    }

    fn sample_mobi(title: &str, text: &str) -> Vec<u8> {
        let title = title.as_bytes();
        let text = text.as_bytes();
        let mut record0 = vec![0u8; 16 + 232];
        record0[0] = 0;
        record0[1] = 1;
        put_u32(&mut record0, 4, u32::try_from(text.len()).unwrap_or(0));
        record0[8] = 0;
        record0[9] = 1;
        record0[10] = 0x10;
        record0[11] = 0;
        record0[16..20].copy_from_slice(b"MOBI");
        put_u32(&mut record0, 20, 232);
        put_u32(&mut record0, 16 + 8, 2);
        put_u32(&mut record0, 16 + 12, 65001);
        let name_at = record0.len();
        record0.extend_from_slice(title);
        put_u32(&mut record0, 16 + 84, u32::try_from(name_at).unwrap_or(0));
        put_u32(
            &mut record0,
            16 + 88,
            u32::try_from(title.len()).unwrap_or(0),
        );
        let record0_at = 78 + 16;
        let text_at = record0_at + record0.len();
        let mut bytes = vec![0u8; 78];
        bytes[60..64].copy_from_slice(b"BOOK");
        bytes[64..68].copy_from_slice(b"MOBI");
        bytes[76] = 0;
        bytes[77] = 2;
        bytes.extend_from_slice(&u32::try_from(record0_at).unwrap_or(0).to_be_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.extend_from_slice(&u32::try_from(text_at).unwrap_or(0).to_be_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.extend_from_slice(&record0);
        bytes.extend_from_slice(text);
        bytes
    }

    fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_be_bytes());
    }
}
