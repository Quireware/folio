//! Entry names: image types, traversal rejection, and natural order.

use std::cmp::Ordering;

use crate::file_extension;

/// JPEG, PNG, WebP, and AVIF page names. Matching is ASCII case-insensitive.
pub(crate) fn is_page_image(name: &str) -> bool {
    let Some(extension) = file_extension(name) else {
        return false;
    };
    let mut folded = [0u8; 4];
    if extension.len() > folded.len() || !extension.is_ascii() {
        return false;
    }
    let bytes = extension.as_bytes();
    for (slot, byte) in folded.iter_mut().zip(bytes) {
        *slot = byte.to_ascii_lowercase();
    }
    let folded = &folded[..bytes.len()];
    matches!(folded, b"jpg" | b"jpeg" | b"png" | b"webp" | b"avif")
}

/// `true` when `name` is absolute or a component is `..`.
pub(crate) fn entry_escapes(name: &str) -> bool {
    if name.starts_with('/') || name.starts_with('\\') {
        return true;
    }
    name.split(['/', '\\'])
        .any(|part| part == ".." || part.contains(':') || part.contains('\0'))
}

/// Digit runs compare by significant length, then lexicographically.
///
/// Leading zeros are ignored for the numeric comparison, so `2` sorts before
/// `10` without converting the run to an integer.
pub(crate) fn natural_cmp(left: &str, right: &str) -> Ordering {
    let mut left_chars = left.chars().peekable();
    let mut right_chars = right.chars().peekable();
    loop {
        match (left_chars.peek().copied(), right_chars.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(left_char), Some(right_char))
                if left_char.is_ascii_digit() && right_char.is_ascii_digit() =>
            {
                let left_run = take_while(&mut left_chars, |character| character.is_ascii_digit());
                let right_run =
                    take_while(&mut right_chars, |character| character.is_ascii_digit());
                let compared = compare_digit_runs(&left_run, &right_run);
                if compared != Ordering::Equal {
                    return compared;
                }
            }
            (Some(left_char), Some(right_char))
                if !left_char.is_ascii_digit() && !right_char.is_ascii_digit() =>
            {
                let left_run = take_while(&mut left_chars, |character| !character.is_ascii_digit());
                let right_run =
                    take_while(&mut right_chars, |character| !character.is_ascii_digit());
                let compared = left_run.cmp(&right_run);
                if compared != Ordering::Equal {
                    return compared;
                }
            }
            (Some(left_char), Some(right_char)) => return left_char.cmp(&right_char),
        }
    }
}

fn compare_digit_runs(left: &str, right: &str) -> Ordering {
    let left_sig = significant_digits(left);
    let right_sig = significant_digits(right);
    left_sig
        .len()
        .cmp(&right_sig.len())
        .then_with(|| left_sig.cmp(right_sig))
}

fn significant_digits(run: &str) -> &str {
    let trimmed = run.trim_start_matches('0');
    if trimmed.is_empty() {
        "0"
    } else {
        trimmed
    }
}

fn take_while(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    predicate: impl Fn(char) -> bool,
) -> String {
    let mut run = String::new();
    while chars.peek().is_some_and(|character| predicate(*character)) {
        let Some(character) = chars.next() else {
            break;
        };
        run.push(character);
    }
    run
}

pub(crate) fn base_name(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use super::natural_cmp;

    #[test]
    fn shorter_digit_run_sorts_first() {
        assert_eq!(natural_cmp("page2.jpg", "page10.jpg"), Ordering::Less);
        assert_eq!(natural_cmp("img001.png", "img2.png"), Ordering::Less);
        assert_eq!(natural_cmp("02.jpg", "2.jpg"), Ordering::Equal);
    }
}
