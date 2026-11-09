//! EPUB CFI points. Formatting is canonical: no leading zeros, no spaces.

use thiserror::Error;

/// One step in a CFI path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CfiStep {
    /// Child index from the CFI path.
    pub index: u32,
    /// Optional `[id]` assertion, without the brackets.
    pub id: Option<String>,
}

/// A parsed EPUB CFI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cfi {
    /// Steps before `!`. The package spine lives here.
    pub spine: Vec<CfiStep>,
    /// Steps after `!`. Empty when the CFI has no indirection.
    pub local: Vec<CfiStep>,
    /// Character offset after `:`, when the CFI has one.
    pub offset: Option<u32>,
}

/// Why a CFI string was rejected.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CfiError {
    /// The string is not an `epubcfi(...)` point.
    #[error("cfi is not an epubcfi point")]
    Invalid,
    /// A step index did not fit in `u32`.
    #[error("cfi step did not fit")]
    Number,
}

/// Parse `input` as one EPUB CFI point.
///
/// # Errors
///
/// Returns an error when `input` is not a single `epubcfi(...)` point this reader stores.
pub fn parse_cfi(input: &str) -> Result<Cfi, CfiError> {
    let inner = input
        .trim()
        .strip_prefix("epubcfi(")
        .and_then(|rest| rest.strip_suffix(')'))
        .ok_or(CfiError::Invalid)?;
    if inner.is_empty() {
        return Err(CfiError::Invalid);
    }
    let (path, offset) = split_offset(inner)?;
    let (spine_src, local_src) = split_indirection(path)?;
    let spine = parse_steps(spine_src)?;
    if spine.is_empty() {
        return Err(CfiError::Invalid);
    }
    let local = if local_src.is_empty() {
        Vec::new()
    } else {
        let local = parse_steps(local_src)?;
        if local.is_empty() {
            return Err(CfiError::Invalid);
        }
        local
    };
    Ok(Cfi {
        spine,
        local,
        offset,
    })
}

/// Canonical form of `cfi`.
pub fn format_cfi(cfi: &Cfi) -> String {
    let mut body = format_steps(&cfi.spine);
    if !cfi.local.is_empty() {
        body.push('!');
        body.push_str(&format_steps(&cfi.local));
    }
    if let Some(offset) = cfi.offset {
        body.push(':');
        body.push_str(&offset.to_string());
    }
    format!("epubcfi({body})")
}

/// Same spine and local path, with `offset` as the character position.
pub fn seek_in_chapter(cfi: &Cfi, offset: u32) -> Cfi {
    // clone: the caller keeps its CFI; seek returns another point
    let mut next = cfi.clone();
    next.offset = Some(offset);
    next
}

/// Replace the last spine step and clear the local path.
///
/// # Errors
///
/// Returns an error when `cfi` has no spine step.
pub fn jump_to_spine_step(cfi: &Cfi, step_index: u32) -> Result<Cfi, CfiError> {
    if cfi.spine.is_empty() {
        return Err(CfiError::Invalid);
    }
    // clone: chapter jump returns a new point and leaves `cfi` in place
    let mut next = cfi.clone();
    if let Some(last) = next.spine.last_mut() {
        last.index = step_index;
        last.id = None;
    }
    next.local.clear();
    next.offset = Some(0);
    Ok(next)
}

fn split_offset(input: &str) -> Result<(&str, Option<u32>), CfiError> {
    let Some(index) = find_unbracketed(input, ':')? else {
        return Ok((input, None));
    };
    let path = &input[..index];
    let after = &input[index + 1..];
    if after.is_empty() || find_unbracketed(after, ':')?.is_some() {
        return Err(CfiError::Invalid);
    }
    if !after.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(CfiError::Invalid);
    }
    let offset = after.parse().map_err(|_| CfiError::Number)?;
    Ok((path, Some(offset)))
}

fn split_indirection(input: &str) -> Result<(&str, &str), CfiError> {
    let Some(index) = find_unbracketed(input, '!')? else {
        return Ok((input, ""));
    };
    let local = &input[index + 1..];
    if local.is_empty() || find_unbracketed(local, '!')?.is_some() {
        return Err(CfiError::Invalid);
    }
    Ok((&input[..index], local))
}

fn find_unbracketed(input: &str, needle: char) -> Result<Option<usize>, CfiError> {
    let mut depth: u32 = 0;
    for (index, ch) in input.char_indices() {
        match ch {
            '[' => {
                depth = depth.checked_add(1).ok_or(CfiError::Invalid)?;
            }
            ']' => {
                depth = depth.checked_sub(1).ok_or(CfiError::Invalid)?;
            }
            found if found == needle && depth == 0 => return Ok(Some(index)),
            _ => {}
        }
    }
    if depth != 0 {
        return Err(CfiError::Invalid);
    }
    Ok(None)
}

fn parse_steps(input: &str) -> Result<Vec<CfiStep>, CfiError> {
    if !input.starts_with('/') {
        return Err(CfiError::Invalid);
    }
    let mut steps = Vec::new();
    for part in input.split('/').skip(1) {
        if part.is_empty() {
            return Err(CfiError::Invalid);
        }
        let (number, id) = match part.find('[') {
            Some(start) => {
                let Some(raw_id) = part[start..]
                    .strip_prefix('[')
                    .and_then(|rest| rest.strip_suffix(']'))
                else {
                    return Err(CfiError::Invalid);
                };
                if raw_id.is_empty() || raw_id.contains(['[', ']', '/', '!', ':']) {
                    return Err(CfiError::Invalid);
                }
                (&part[..start], Some(raw_id.to_owned()))
            }
            None => (part, None),
        };
        if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(CfiError::Invalid);
        }
        let index = number.parse().map_err(|_| CfiError::Number)?;
        steps.push(CfiStep { index, id });
    }
    Ok(steps)
}

fn format_steps(steps: &[CfiStep]) -> String {
    let mut out = String::new();
    for step in steps {
        out.push('/');
        out.push_str(&step.index.to_string());
        if let Some(id) = &step.id {
            out.push('[');
            out.push_str(id);
            out.push(']');
        }
    }
    out
}
