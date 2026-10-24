//! Text-layer preservation, reader half (task 155).
//!
//! A text layer's `TySh` (or legacy `tySh`) tagged block is kept byte for
//! byte, and so is the document-level `Txt2` block (the text engine's
//! document resources). Nothing in either is interpreted, except that the
//! names of the fonts a layer references are read out of the layer's engine
//! data **for the report only** (best effort: a layer whose data does not
//! look as expected simply lists no fonts). See `aurora_doc::text_layer` for
//! the model and the policy for when a save drops the data.
//!
//! **Bounded against hostile input.** A layer's block is limited to
//! [`MAX_TEXT_BLOCK_BYTES`], all layers' blocks together to
//! [`aurora_doc::MAX_TEXT_TOTAL_BYTES`], and the `Txt2` block to
//! [`MAX_TEXT_ENGINE_BYTES`]; a block over its bound is not kept (the layer
//! opens as pixels and the open reports it). The font scan is linear in the
//! block and keeps at most [`MAX_FONTS_PER_LAYER`] names of
//! [`MAX_FONT_NAME_CHARS`] characters.

use aurora_doc::{MAX_TEXT_BLOCK_BYTES, MAX_TEXT_ENGINE_BYTES, TEXT_BLOCK_KEYS};

use super::{ByteSource, IoError, Record, Window};

/// The most font names read from one layer.
pub(super) const MAX_FONTS_PER_LAYER: usize = 32;

/// The longest font name kept, in characters.
pub(super) const MAX_FONT_NAME_CHARS: usize = 64;

/// The most distinct font names the report lists.
pub(super) const MAX_FONTS_REPORTED: usize = 12;

/// A layer's text block as the reader kept it, ready for
/// `aurora_doc::TextLayer` once the pixels give it its fingerprint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PsdTextLayer {
    pub key: [u8; 4],
    pub data: Vec<u8>,
}

/// A block's data if it is one of the kept kinds, long enough to hold the
/// fixed head Photoshop writes (version, transform, text version,
/// descriptor version), within the per-block bound **and within what is
/// left of the file-wide bound** (`budget`, which starts at
/// [`aurora_doc::MAX_TEXT_TOTAL_BYTES`] and is charged for every block copied). The
/// copy is only made after those checks, so a hostile file with thousands
/// of records never holds more than the bound, whatever its size.
pub(super) fn candidate(key: [u8; 4], data: &[u8], budget: &mut usize) -> Option<PsdTextLayer> {
    // `TySh`: version (2), transform (48), text version (2), descriptor
    // version (4). The legacy `tySh` has the same version and transform.
    const HEAD: usize = 2 + 48 + 2 + 4;
    let ok = TEXT_BLOCK_KEYS.contains(&key)
        && data.len() >= HEAD
        && data.len() <= MAX_TEXT_BLOCK_BYTES
        && data.len() <= *budget
        && data.starts_with(&[0, 1]);
    if !ok {
        return None;
    }
    *budget -= data.len();
    Some(PsdTextLayer {
        key,
        data: data.to_vec(),
    })
}

/// Promotes the candidates to [`Record::text`], except those on a group's
/// records. The file-wide bound was applied while the records were read
/// ([`candidate`]), so nothing larger than it was ever copied.
pub(super) fn resolve(records: &mut [Record]) {
    for record in records.iter_mut() {
        let Some(candidate) = record.text_candidate.take() else {
            continue;
        };
        if record.section.is_none() {
            record.text = Some(candidate);
        }
    }
}

/// Reads the `Txt2` block if it is within bounds.
///
/// # Errors
///
/// [`IoError`] from the read.
pub(super) fn read_engine(
    src: &mut dyn ByteSource,
    window: Option<Window>,
) -> Result<Option<std::sync::Arc<Vec<u8>>>, IoError> {
    let Some(window) = window else {
        return Ok(None);
    };
    let len = window.remaining();
    if len == 0 || len > MAX_TEXT_ENGINE_BYTES as u64 {
        return Ok(None);
    }
    let Ok(len) = usize::try_from(len) else {
        return Ok(None);
    };
    Ok(Some(std::sync::Arc::new(window.peek(
        src,
        len,
        "text engine data",
    )?)))
}

/// The font names a `TySh` block's engine data references, in order, for
/// the report. Empty when the data does not look as expected.
pub(super) fn font_names(block: &[u8]) -> Vec<String> {
    let Some(engine) = engine_data(block) else {
        return Vec::new();
    };
    font_set_names(engine)
}

/// The `EngineData` raw item of the text descriptor: the key, the `tdta`
/// type, a `u32` length and that many bytes.
fn engine_data(block: &[u8]) -> Option<&[u8]> {
    const NEEDLE: &[u8] = b"EngineDatatdta";
    let at = block.windows(NEEDLE.len()).position(|w| w == NEEDLE)? + NEEDLE.len();
    let len = u32::from_be_bytes(block.get(at..at + 4)?.try_into().ok()?);
    let start = at + 4;
    block.get(start..start.checked_add(usize::try_from(len).ok()?)?)
}

/// The `/Name (...)` strings inside the first `/FontSet [ ... ]`.
fn font_set_names(engine: &[u8]) -> Vec<String> {
    const SET: &[u8] = b"/FontSet";
    let Some(mut i) = engine
        .windows(SET.len())
        .position(|w| w == SET)
        .map(|p| p + SET.len())
    else {
        return Vec::new();
    };
    // Up to the opening bracket; anything else first means a layout this
    // scan does not know.
    while engine.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    if engine.get(i) != Some(&b'[') {
        return Vec::new();
    }
    let mut depth = 0usize;
    let mut names = Vec::new();
    let mut after_name_key = false;
    while let Some(&b) = engine.get(i) {
        match b {
            b'[' => depth += 1,
            b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            b'(' => {
                let (text, next) = string_at(engine, i);
                if after_name_key && names.len() < MAX_FONTS_PER_LAYER {
                    let name = decode_name(&text);
                    if !name.is_empty() {
                        names.push(name);
                    }
                }
                after_name_key = false;
                i = next;
                continue;
            }
            b'/' => {
                let end = engine.get(i + 1..).map_or(0, |rest| {
                    rest.iter()
                        .position(|c| {
                            c.is_ascii_whitespace()
                                || matches!(c, b'/' | b'(' | b'<' | b'>' | b'[' | b']')
                        })
                        .unwrap_or(rest.len())
                });
                after_name_key = engine.get(i + 1..i + 1 + end) == Some(b"Name");
                i += 1 + end;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    names
}

/// The bytes of the parenthesised string starting at `at` (escapes
/// resolved) and the index just past its closing parenthesis.
fn string_at(data: &[u8], at: usize) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut i = at;
    while let Some(&b) = data.get(i) {
        i += 1;
        match b {
            b'\\' => {
                if let Some(&escaped) = data.get(i) {
                    out.push(escaped);
                    i += 1;
                }
            }
            b'(' => {
                depth += 1;
                if depth > 1 {
                    out.push(b);
                }
            }
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return (out, i);
                }
                out.push(b);
            }
            _ => out.push(b),
        }
    }
    (out, i)
}

/// A font name from a string's bytes: UTF-16BE after a byte-order mark,
/// else Latin-1; control characters dropped, length bounded.
fn decode_name(bytes: &[u8]) -> String {
    let text: String = match bytes.strip_prefix(&[0xFE, 0xFF]) {
        Some(rest) => char::decode_utf16(rest.chunks_exact(2).map(|p| match p {
            [a, b] => u16::from_be_bytes([*a, *b]),
            _ => 0,
        }))
        .map(|c| c.unwrap_or('\u{FFFD}'))
        .collect(),
        None => bytes.iter().map(|&b| char::from(b)).collect(),
    };
    let cleaned: String = text
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_FONT_NAME_CHARS)
        .collect();
    aurora_doc::sanitize_display_name(cleaned.trim()).into_owned()
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)] // fixed-size fixtures
mod tests {
    use super::*;
    use aurora_doc::MAX_TEXT_TOTAL_BYTES;

    fn utf16(name: &str) -> Vec<u8> {
        let mut out = vec![0xFE, 0xFF];
        for u in name.encode_utf16() {
            out.extend_from_slice(&u.to_be_bytes());
        }
        out
    }

    fn engine_with(fonts: &[&str]) -> Vec<u8> {
        let mut e = b"\n<<\n\t/ResourceDict\n\t<<\n\t\t/FontSet [\n".to_vec();
        for f in fonts {
            e.extend_from_slice(b"\t\t<<\n\t\t\t/Name (");
            for &b in &utf16(f) {
                if matches!(b, b'(' | b')' | b'\\') {
                    e.push(b'\\');
                }
                e.push(b);
            }
            e.extend_from_slice(b")\n\t\t\t/Script 0\n\t\t>>\n");
        }
        e.extend_from_slice(b"\t\t]\n\t\t/Other (x)\n\t>>\n>>");
        e
    }

    fn block_with(engine: &[u8]) -> Vec<u8> {
        let mut b = vec![0, 1];
        b.extend_from_slice(&[0; 48]);
        b.extend_from_slice(&[0, 50, 0, 0, 0, 16]);
        b.extend_from_slice(b"junk EngineDatatdta");
        b.extend_from_slice(&(engine.len() as u32).to_be_bytes());
        b.extend_from_slice(engine);
        b.extend_from_slice(&[0; 4]);
        b
    }

    #[test]
    fn fonts_are_read_from_the_font_set_only() {
        let block = block_with(&engine_with(&["ArialMT", "Myriad Pro-Regular"]));
        assert_eq!(font_names(&block), vec!["ArialMT", "Myriad Pro-Regular"]);
    }

    #[test]
    fn escaped_parentheses_and_non_latin_names_survive() {
        let block = block_with(&engine_with(&["A(B)C", "Борис"]));
        assert_eq!(font_names(&block), vec!["A(B)C", "Борис"]);
    }

    #[test]
    fn a_name_key_outside_the_font_set_is_not_a_font() {
        let mut e = b"/Name (decoy) /FontSet [ ] /Name (decoy2)".to_vec();
        assert!(font_names(&block_with(&e)).is_empty());
        e = b"/FontSet [ << /Other (x) /Name (Real) >> ] /Name (decoy)".to_vec();
        assert_eq!(font_names(&block_with(&e)), vec!["Real"]);
    }

    #[test]
    fn data_that_does_not_look_right_lists_no_fonts_and_never_panics() {
        assert!(font_names(&[]).is_empty());
        assert!(font_names(b"no engine data here").is_empty());
        // A length that runs past the block.
        let mut b = b"EngineDatatdta".to_vec();
        b.extend_from_slice(&u32::MAX.to_be_bytes());
        b.extend_from_slice(b"/FontSet [ << /Name (x");
        assert!(font_names(&b).is_empty());
        // An unterminated string and list.
        assert!(font_names(&block_with(b"/FontSet [ << /Name (abc")).len() <= 1);
        // FontSet not followed by a list.
        assert!(font_names(&block_with(b"/FontSet 7 /Name (x)")).is_empty());
    }

    #[test]
    fn the_number_and_length_of_names_are_bounded() {
        let many: Vec<String> = (0..100).map(|i| format!("Font{i}")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(
            font_names(&block_with(&engine_with(&refs))).len(),
            MAX_FONTS_PER_LAYER
        );
        let long = "x".repeat(500);
        let names = font_names(&block_with(&engine_with(&[&long])));
        assert_eq!(names[0].chars().count(), MAX_FONT_NAME_CHARS);
    }

    #[test]
    fn candidates_need_a_kept_key_a_head_and_the_bound() {
        let good = block_with(&engine_with(&["A"]));
        let fresh = || MAX_TEXT_TOTAL_BYTES;
        assert!(candidate(*b"TySh", &good, &mut fresh()).is_some());
        assert!(candidate(*b"tySh", &good, &mut fresh()).is_some());
        assert!(candidate(*b"SoLd", &good, &mut fresh()).is_none());
        assert!(
            candidate(*b"TySh", &good[..50], &mut fresh()).is_none(),
            "no head"
        );
        let mut wrong_version = good.clone();
        wrong_version[1] = 2;
        assert!(candidate(*b"TySh", &wrong_version, &mut fresh()).is_none());
        let mut big = vec![0, 1];
        big.resize(MAX_TEXT_BLOCK_BYTES, 0);
        assert!(
            candidate(*b"TySh", &big, &mut fresh()).is_some(),
            "the bound itself"
        );
        big.push(0);
        assert!(
            candidate(*b"TySh", &big, &mut fresh()).is_none(),
            "one over"
        );
    }

    #[test]
    fn the_budget_is_charged_and_nothing_past_it_is_copied() {
        let mut block = block_with(&engine_with(&["A"]));
        block.resize(MAX_TEXT_BLOCK_BYTES, 0);
        let mut budget = MAX_TEXT_TOTAL_BYTES;
        let mut copied = 0usize;
        let mut kept = 0usize;
        for _ in 0..1000 {
            if let Some(c) = candidate(*b"TySh", &block, &mut budget) {
                copied += c.data.len();
                kept += 1;
            }
        }
        assert_eq!(kept, MAX_TEXT_TOTAL_BYTES / MAX_TEXT_BLOCK_BYTES);
        assert_eq!(copied, MAX_TEXT_TOTAL_BYTES, "exactly the bound, no more");
        assert_eq!(budget, 0);
        // A block that does not fit leaves the budget alone; a smaller one
        // after it still fits.
        let small = block_with(&engine_with(&[]));
        let mut budget = small.len() + 7;
        assert!(candidate(*b"TySh", &block, &mut budget).is_none());
        assert_eq!(budget, small.len() + 7);
        assert!(candidate(*b"TySh", &small, &mut budget).is_some());
        assert_eq!(budget, 7);
    }
}
