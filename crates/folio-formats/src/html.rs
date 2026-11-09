//! Chapter HTML is kept as text with scripts, styles, and remote schemes removed.

/// Drop `<script>` and `<style>` elements, then `http://` and `https://`.
///
/// The result contains neither `<script` nor `http`, ASCII case-insensitively,
/// for input whose only `http` sequences were those schemes or lived inside
/// the removed elements.
pub(crate) fn strip_chapter_html(input: &str) -> String {
    finish(drop_elements(input, &[b"script", b"style"]))
}

/// HTML that may be placed in the content frame.
///
/// Scripts, styles, forms, `on*` attributes, and remote `http`/`https` values
/// are removed. Loopback URLs are added by the server after this returns, so
/// they are not stripped here.
pub fn sanitize_chapter_html(input: &str) -> String {
    let dropped = drop_elements(input, &[b"script", b"style", b"form"]);
    finish(strip_attributes(&dropped, false))
}

/// Kavita page HTML. Scripts and event handlers are removed. Image URLs stay.
pub fn sanitize_kavita_html(input: &str) -> String {
    let dropped = drop_elements(input, &[b"script", b"style", b"form"]);
    strip_attributes(&dropped, true)
}

fn drop_elements(input: &str, names: &[&[u8]]) -> String {
    let folded = input.to_ascii_lowercase();
    debug_assert_eq!(folded.len(), input.len());
    let raw = input.as_bytes();
    let folded_bytes = folded.as_bytes();
    let mut kept = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        let mut skipped = false;
        for name in names {
            if element_opens_at(folded_bytes, index, name) {
                index = skip_element(folded_bytes, index, name);
                skipped = true;
                break;
            }
        }
        if skipped {
            continue;
        }
        kept.push(raw[index]);
        index = index.saturating_add(1);
    }
    String::from_utf8(kept)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

fn finish(text: String) -> String {
    let stripped = without_schemes(text.as_bytes());
    String::from_utf8(stripped)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

fn element_opens_at(folded: &[u8], index: usize, name: &[u8]) -> bool {
    let Some(rest) = folded.get(index..) else {
        return false;
    };
    if rest.first() != Some(&b'<') {
        return false;
    }
    let Some(after) = rest.get(1..) else {
        return false;
    };
    let Some(tail) = after.get(name.len()..) else {
        return false;
    };
    if &after[..name.len()] != name {
        return false;
    }
    matches!(
        tail.first(),
        Some(b'/' | b'>' | b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')
    )
}

fn skip_element(folded: &[u8], index: usize, name: &[u8]) -> usize {
    let open_end = find_tag_end(folded, index).unwrap_or(folded.len());
    let mut marker = Vec::with_capacity(name.len().saturating_add(2));
    marker.extend_from_slice(b"</");
    marker.extend_from_slice(name);
    let Some(relative) = find_bytes(folded.get(open_end..).unwrap_or_default(), &marker) else {
        return folded.len();
    };
    let close_at = open_end.saturating_add(relative);
    find_tag_end(folded, close_at)
        .map(|end| end.saturating_add(1))
        .unwrap_or(folded.len())
}

fn find_tag_end(bytes: &[u8], from: usize) -> Option<usize> {
    let mut quoted: Option<u8> = None;
    for (offset, byte) in bytes.get(from..)?.iter().enumerate() {
        if let Some(quote) = quoted {
            if *byte == quote {
                quoted = None;
            }
            continue;
        }
        match byte {
            b'"' | b'\'' => quoted = Some(*byte),
            b'>' => return from.checked_add(offset),
            _ => {}
        }
    }
    None
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn strip_attributes(input: &str, allow_remote: bool) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'<' {
            out.push(bytes[index]);
            index = index.saturating_add(1);
            continue;
        }
        let Some(end) = find_tag_end(bytes, index) else {
            out.extend_from_slice(&bytes[index..]);
            break;
        };
        out.extend(clean_tag(&bytes[index..=end], allow_remote));
        index = end.saturating_add(1);
    }
    String::from_utf8(out)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

fn clean_tag(tag: &[u8], allow_remote: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(tag.len());
    let mut index = 0;
    while index < tag.len() && !tag[index].is_ascii_whitespace() && tag[index] != b'>' {
        out.push(tag[index]);
        index = index.saturating_add(1);
    }
    while index < tag.len() {
        while index < tag.len() && tag[index].is_ascii_whitespace() {
            index = index.saturating_add(1);
        }
        if index >= tag.len() || tag[index] == b'>' || tag[index] == b'/' {
            break;
        }
        let name_start = index;
        while index < tag.len()
            && tag[index] != b'='
            && tag[index] != b'>'
            && tag[index] != b'/'
            && !tag[index].is_ascii_whitespace()
        {
            index = index.saturating_add(1);
        }
        let name = &tag[name_start..index];
        while index < tag.len() && tag[index].is_ascii_whitespace() {
            index = index.saturating_add(1);
        }
        let value = if index < tag.len() && tag[index] == b'=' {
            index = index.saturating_add(1);
            while index < tag.len() && tag[index].is_ascii_whitespace() {
                index = index.saturating_add(1);
            }
            read_attribute_value(tag, &mut index)
        } else {
            None
        };
        if keep_attribute(name, value.as_deref(), allow_remote) {
            out.push(b' ');
            out.extend_from_slice(name);
            if let Some(value) = value {
                out.push(b'=');
                out.extend_from_slice(&value);
            }
        }
    }
    if tag.windows(2).any(|window| window == b"/>") && tag.ends_with(b"/>") {
        out.extend_from_slice(b"/>");
    } else if tag.last() == Some(&b'>') {
        out.push(b'>');
    }
    out
}

fn read_attribute_value(tag: &[u8], index: &mut usize) -> Option<Vec<u8>> {
    let start = *index;
    let quote = tag.get(*index).copied();
    if matches!(quote, Some(b'"' | b'\'')) {
        *index = index.saturating_add(1);
        while *index < tag.len() && tag.get(*index) != quote.as_ref() {
            *index = index.saturating_add(1);
        }
        if *index < tag.len() {
            *index = index.saturating_add(1);
        }
        return Some(tag[start..*index].to_vec());
    }
    while *index < tag.len() && !tag[*index].is_ascii_whitespace() && tag[*index] != b'>' {
        *index = index.saturating_add(1);
    }
    Some(tag[start..*index].to_vec())
}

fn keep_attribute(name: &[u8], value: Option<&[u8]>, allow_remote: bool) -> bool {
    if name.is_empty() {
        return false;
    }
    let folded = name.to_ascii_lowercase();
    if folded.starts_with(b"on")
        && folded.len() > 2
        && folded[2..].iter().all(u8::is_ascii_alphabetic)
    {
        return false;
    }
    let Some(value) = value else {
        return true;
    };
    let folded_value = value.to_ascii_lowercase();
    if folded_value
        .windows(11)
        .any(|window| window == b"javascript:")
    {
        return false;
    }
    if allow_remote {
        return true;
    }
    !folded_value.windows(8).any(|window| window == b"https://")
        && !folded_value.windows(7).any(|window| window == b"http://")
}

fn without_schemes(bytes: &[u8]) -> Vec<u8> {
    let folded = bytes.to_ascii_lowercase();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if folded
            .get(index..)
            .is_some_and(|rest| rest.starts_with(b"https://"))
        {
            index = index.saturating_add(8);
            continue;
        }
        if folded
            .get(index..)
            .is_some_and(|rest| rest.starts_with(b"http://"))
        {
            index = index.saturating_add(7);
            continue;
        }
        out.push(bytes[index]);
        index = index.saturating_add(1);
    }
    out
}
