//! A reader/writer for `EngineData` — the plain-text-ish markup Photoshop's
//! text engine (ATE) uses for paragraph/style/resource data, embedded as the
//! raw payload of `TySh`'s `EngineData` field (see `descriptor.rs`, `Value::Raw`).
//!
//! Unlike the Descriptor binary format, this one is a human-legible nested
//! structure — `<<...>>` dicts, `/Key` properties, `[...]` lists, numbers,
//! `true`/`false`, and parenthesized strings — but the string encoding is not
//! plain text: each string is a UTF-16BE byte-order mark followed by
//! UTF-16BE-encoded text, with `\`, `(`, `)` backslash-escaped.
//!
//! Every construct here was confirmed by reading `psd-tools`' own tokenizer
//! and writer (`psd/engine_data.py`) before writing this file, then tested
//! against the `EngineData` payload actually embedded in `reference/tysh.bin`
//! (extracted from a genuine Photoshop-authored file). `reference/engineData.txt`
//! (from `ag-psd`'s test suite) was useful for confirming the *structure* —
//! keys, nesting shape — while designing this, but turned out to be a
//! human-readable pretty-print with no embedded UTF-16BE/BOM bytes at all,
//! not the real wire format; see `reference/README.md`. Caught by checking
//! its raw bytes directly rather than assuming a plausible-looking reference
//! file was usable as real test input.
//!
//! See FINDINGS.md findings 6 and 9 for why reading a working implementation
//! mattered here specifically, rather than working from spec text alone.

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Dict(Vec<(String, Value)>),
    List(Vec<Value>),
    Str(String),
    Bool(bool),
    Int(i64),
    Float(f64),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Dict(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        match self {
            Value::Dict(items) => items.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Walks a dotted path, e.g. `get_path("EngineDict.Editor.Text")`.
    pub fn get_path(&self, path: &str) -> Option<&Value> {
        let mut cur = self;
        for seg in path.split('.') {
            cur = cur.get(seg)?;
        }
        Some(cur)
    }

    pub fn get_path_mut(&mut self, path: &str) -> Option<&mut Value> {
        let mut cur = self;
        for seg in path.split('.') {
            cur = cur.get_mut(seg)?;
        }
        Some(cur)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq)]
enum Token {
    DictStart,
    DictEnd,
    ArrayStart,
    ArrayEnd,
    Property(String),
    Str(String),
    Bool(bool),
    Int(i64),
    Float(f64),
}

struct Tokenizer<'a> {
    data: &'a [u8],
    pos: usize,
}

#[derive(Debug)]
pub struct ParseError(String);
impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EngineData parse error: {}", self.0)
    }
}
impl std::error::Error for ParseError {}

type Result<T> = std::result::Result<T, ParseError>;

fn err(msg: impl Into<String>) -> ParseError {
    ParseError(msg.into())
}

impl<'a> Tokenizer<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.data.len() && self.data[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    /// A parenthesized string: `(` + UTF-16BE BOM (FE FF) + UTF-16BE text,
    /// with `\`, `(`, `)` backslash-escaped + `)`. Confirmed against
    /// `psd-tools`' `engine_data.String.frombytes`/`write`.
    fn read_string(&mut self) -> Result<String> {
        debug_assert_eq!(self.peek(), Some(b'('));
        self.pos += 1;
        let mut raw = Vec::new();
        loop {
            match self.data.get(self.pos) {
                None => return Err(err("unterminated string")),
                Some(b'\\') => {
                    let next = *self
                        .data
                        .get(self.pos + 1)
                        .ok_or_else(|| err("dangling escape at end of input"))?;
                    raw.push(next);
                    self.pos += 2;
                }
                Some(b')') => {
                    self.pos += 1;
                    break;
                }
                Some(&b) => {
                    raw.push(b);
                    self.pos += 1;
                }
            }
        }
        if raw.len() < 2 || raw[0] != 0xFE || raw[1] != 0xFF {
            return Err(err("string missing UTF-16BE byte-order mark"));
        }
        let units: Vec<u16> = raw[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16(&units).map_err(|e| err(format!("invalid UTF-16: {e}")))
    }

    fn read_bareword(&mut self) -> Result<Token> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b.is_ascii_whitespace() || matches!(b, b'<' | b'>' | b'[' | b']' | b'/' | b'(') {
                break;
            }
            self.pos += 1;
        }
        let word = std::str::from_utf8(&self.data[start..self.pos])
            .map_err(|_| err("non-UTF8 bareword"))?;
        match word {
            "true" => Ok(Token::Bool(true)),
            "false" => Ok(Token::Bool(false)),
            _ => {
                if let Ok(i) = word.parse::<i64>() {
                    Ok(Token::Int(i))
                } else if let Ok(f) = word.parse::<f64>() {
                    Ok(Token::Float(f))
                } else {
                    Err(err(format!("unrecognized token {word:?}")))
                }
            }
        }
    }

    fn next(&mut self) -> Result<Option<Token>> {
        self.skip_ws();
        let Some(b) = self.peek() else {
            return Ok(None);
        };
        match b {
            b'<' => {
                self.expect_seq(b"<<")?;
                Ok(Some(Token::DictStart))
            }
            b'>' => {
                self.expect_seq(b">>")?;
                Ok(Some(Token::DictEnd))
            }
            b'[' => {
                self.pos += 1;
                Ok(Some(Token::ArrayStart))
            }
            b']' => {
                self.pos += 1;
                Ok(Some(Token::ArrayEnd))
            }
            b'/' => {
                self.pos += 1;
                let start = self.pos;
                while let Some(c) = self.peek() {
                    if c.is_ascii_whitespace()
                        || matches!(c, b'<' | b'>' | b'[' | b']' | b'/' | b'(')
                    {
                        break;
                    }
                    self.pos += 1;
                }
                let name = std::str::from_utf8(&self.data[start..self.pos])
                    .map_err(|_| err("non-UTF8 property name"))?
                    .to_string();
                Ok(Some(Token::Property(name)))
            }
            b'(' => Ok(Some(Token::Str(self.read_string()?))),
            _ => Ok(Some(self.read_bareword()?)),
        }
    }

    fn expect_seq(&mut self, seq: &[u8]) -> Result<()> {
        if self.data[self.pos..].starts_with(seq) {
            self.pos += seq.len();
            Ok(())
        } else {
            Err(err(format!(
                "expected {:?} at byte {}",
                std::str::from_utf8(seq).unwrap_or("?"),
                self.pos
            )))
        }
    }
}

pub fn parse(data: &[u8]) -> Result<Value> {
    let mut t = Tokenizer::new(data);
    let first = t.next()?.ok_or_else(|| err("empty input"))?;
    parse_from(&mut t, first)
}

fn parse_from(t: &mut Tokenizer, first: Token) -> Result<Value> {
    match first {
        Token::DictStart => parse_dict(t),
        Token::ArrayStart => parse_list(t),
        Token::Str(s) => Ok(Value::Str(s)),
        Token::Bool(b) => Ok(Value::Bool(b)),
        Token::Int(i) => Ok(Value::Int(i)),
        Token::Float(f) => Ok(Value::Float(f)),
        other => Err(err(format!("unexpected token at top level: {other:?}"))),
    }
}

fn parse_dict(t: &mut Tokenizer) -> Result<Value> {
    let mut items = Vec::new();
    loop {
        match t.next()? {
            None => return Err(err("unterminated dict")),
            Some(Token::DictEnd) => return Ok(Value::Dict(items)),
            Some(Token::Property(name)) => {
                let value_tok = t.next()?.ok_or_else(|| err("dict key with no value"))?;
                let value = parse_from(t, value_tok)?;
                items.push((name, value));
            }
            Some(other) => return Err(err(format!("expected /Property or >>, got {other:?}"))),
        }
    }
}

fn parse_list(t: &mut Tokenizer) -> Result<Value> {
    let mut items = Vec::new();
    loop {
        match t.next()? {
            None => return Err(err("unterminated array")),
            Some(Token::ArrayEnd) => return Ok(Value::List(items)),
            Some(tok) => items.push(parse_from(t, tok)?),
        }
    }
}

fn write_string(out: &mut Vec<u8>, s: &str) {
    // Matches `psd-tools`' own escaping exactly: encode to UTF-16BE, then do
    // a naive whole-buffer byte-level replace for `\`, `(`, `)`, in that
    // order (backslash first, so bytes inserted while escaping `(`/`)` are
    // never re-escaped by a later pass). This deliberately does NOT try to
    // be "smarter" by only escaping ASCII-range low bytes -- an earlier
    // version of this function did that and was a real bug: some non-ASCII
    // codepoints (e.g. U+29xx) have 0x29 as their *high* byte, and an
    // unescaped literal `)` byte there would be misread by `read_string` as
    // the string's terminator, truncating the string silently. Byte-level
    // escaping, unit-alignment-unaware, is what actually round-trips for
    // arbitrary Unicode content.
    let mut encoded: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
    for special in *b"\\()" {
        let mut escaped = Vec::with_capacity(encoded.len());
        for &b in &encoded {
            if b == special {
                escaped.push(b'\\');
            }
            escaped.push(b);
        }
        encoded = escaped;
    }
    out.push(b'(');
    out.push(0xFE);
    out.push(0xFF);
    out.extend_from_slice(&encoded);
    out.push(b')');
}

fn write_float(out: &mut Vec<u8>, f: f64) {
    // Matches psd-tools' formatting: 8 decimals, trim trailing zeros, and a
    // magnitude between 0 and 1 drops the leading "0" (".5" not "0.5"). Not
    // required for a reader to accept the value, but keeping it close to
    // Photoshop's own output style reduces risk for no cost.
    let mut s = format!("{f:.8}");
    while s.ends_with('0') && !s.ends_with(".0") {
        s.pop();
    }
    if f.abs() < 1.0 && f != 0.0 {
        if let Some(stripped) = s.strip_prefix('0') {
            s = stripped.to_string();
        } else if let Some(stripped) = s.strip_prefix("-0") {
            s = format!("-{stripped}");
        }
    }
    out.extend_from_slice(s.as_bytes());
}

// `psd-tools`' own tokenizer (engine_data.py `Tokenizer`) splits tokens on
// whitespace ONLY (`[ \n\t]+`) — unlike this file's reader, which also
// treats `<`, `>`, `[`, `]`, `/`, `(` as delimiters and so tolerates zero
// whitespace between tokens. A real Photoshop-authored file always has
// whitespace between every token (confirmed directly: reference/tysh.bin's
// raw bytes are `\n\n<<\n\t/EngineDict\n\t<<...`, not `<</EngineDict...`).
// Every token written here must therefore be separated by at least one
// space, or a stricter/independent reader reads two adjacent tokens as one
// unrecognizable blob and fails outright — caught exactly this way, by
// embedding a patched file in a real PSD and having psd-tools reject it
// with "Unknown token: b'<</EngineDict'", not by inspection or by our own
// (too permissive) round-trip tests, which this bug passed cleanly. See
// FINDINGS.md finding 14.
fn write_value(out: &mut Vec<u8>, v: &Value) {
    match v {
        Value::Dict(items) => {
            out.extend_from_slice(b"<<");
            for (k, val) in items {
                out.push(b' ');
                out.push(b'/');
                out.extend_from_slice(k.as_bytes());
                out.push(b' ');
                write_value(out, val);
            }
            out.push(b' ');
            out.extend_from_slice(b">>");
        }
        Value::List(items) => {
            out.push(b'[');
            for item in items {
                out.push(b' ');
                write_value(out, item);
            }
            out.push(b' ');
            out.push(b']');
        }
        Value::Str(s) => write_string(out, s),
        Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Value::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Value::Float(f) => write_float(out, *f),
    }
}

pub fn write(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write_value(&mut out, v);
    out
}

/// Recomputes `EngineDict.ParagraphRun`/`StyleRun`'s `RunLengthArray`s so
/// they sum to `new_len` (UTF-16 code units — Photoshop's own unit for these,
/// confirmed against `text-test.psd`'s two-run fixture: `RunLengthArray`
/// always sums to `EngineDict.Editor.Text`'s UTF-16 length, never its scalar
/// character count).
///
/// **Scope, deliberate:** collapses each run array down to its *first*
/// existing entry (reusing that entry's `ParagraphSheet`/`StyleSheet`
/// formatting) and gives it the whole new length. That's correct exactly
/// when an edit replaces the entire text with a single paragraph in a
/// single style — which is what `--tysh-demo` actually does, and the case
/// finding 10 in FINDINGS.md named as still-broken. Preserving multiple
/// paragraph/style runs *across* an edit (e.g. inserting a character in the
/// middle of a multi-style word) needs a real cursor/selection model over
/// the text, which this patch-in-place spike doesn't have — that's Aurora's
/// own text-editing engine's job in Phase 3, not this exercise. See
/// FINDINGS.md finding 12.
pub fn recompute_run_lengths(engine: &mut Value, new_len: usize) -> Result<()> {
    for run_key in ["ParagraphRun", "StyleRun"] {
        let path = format!("EngineDict.{run_key}");
        let run = engine
            .get_path_mut(&path)
            .ok_or_else(|| err(format!("{path} missing")))?;
        let Value::Dict(items) = run else {
            return Err(err(format!("{path} is not a dict")));
        };

        match items.iter_mut().find(|(k, _)| k == "RunArray") {
            Some((_, Value::List(run_array))) if !run_array.is_empty() => {
                run_array.truncate(1);
            }
            _ => {
                return Err(err(format!(
                    "{path}.RunArray missing, empty, or not a list"
                )));
            }
        }
        match items.iter_mut().find(|(k, _)| k == "RunLengthArray") {
            Some((_, v @ Value::List(_))) => {
                *v = Value::List(vec![Value::Int(new_len as i64)]);
            }
            _ => return Err(err(format!("{path}.RunLengthArray missing or not a list"))),
        }
    }
    Ok(())
}

/// A text run's rendering-relevant style: font size (the document's own
/// point size — used directly as `glyph::rasterize`'s `font_size_px`, no
/// further scaling needed, since the real fixture's own `TySh.transform`
/// carries no additional scale factor) and fill color.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunStyle {
    pub font_size: f32,
    pub color: (u8, u8, u8),
}

/// Reads the *first* style run's `FontSize`/`FillColor` from
/// `EngineDict.StyleRun.RunArray[0].StyleSheet.StyleSheetData` — the
/// counterpart to `recompute_run_lengths`'s same "first run" scope: correct
/// for the one edit shape this spike supports (whole-text replacement,
/// single style), not general multi-run styling.
///
/// Confirmed against a real fixture that `RunArray[0].StyleSheet
/// .StyleSheetData` carries `FontSize`/`FillColor` directly, not only on
/// `DefaultRunData`, before relying on that path. See FINDINGS.md finding 15.
pub fn first_run_style(engine: &Value) -> Result<RunStyle> {
    let Some(Value::List(run_array)) = engine.get_path("EngineDict.StyleRun.RunArray") else {
        return Err(err("EngineDict.StyleRun.RunArray missing or not a list"));
    };
    let first = run_array
        .first()
        .ok_or_else(|| err("EngineDict.StyleRun.RunArray is empty"))?;

    let font_size = match first.get_path("StyleSheet.StyleSheetData.FontSize") {
        Some(Value::Float(f)) => *f as f32,
        Some(Value::Int(i)) => *i as f32,
        other => return Err(err(format!("FontSize missing or not numeric: {other:?}"))),
    };

    let fill_color = first
        .get_path("StyleSheet.StyleSheetData.FillColor")
        .ok_or_else(|| err("FillColor missing"))?;
    let color = decode_fill_color(fill_color)?;

    Ok(RunStyle { font_size, color })
}

/// Decodes a `FillColor`/`StrokeColor`-shaped `{Type, Values}` dict into
/// straight RGB. Confirmed against `ag-psd`'s own *encoder* (`text.ts`'s
/// `encodeColor`), since `psd-tools` doesn't model this field at all and
/// treats it as opaque: `Type: 1` is RGB, `Values: [alpha_or_1, R, G, B]` in
/// 0.0-1.0 floats; `Type: 0` is grayscale, `Values: [1, K]`; `Type: 2` is
/// CMYK, `Values: [1, C, M, Y, K]`. See FINDINGS.md finding 13's note on this
/// and finding 15.
///
/// Alpha (`Values[0]` for the RGB case) is intentionally not applied —
/// `glyph::rasterize` takes a straight, fully-opaque color; genuine alpha
/// compositing for a translucent fill is a separate, unstarted concern, the
/// same scope boundary `glyph.rs`'s module docs already note.
fn decode_fill_color(v: &Value) -> Result<(u8, u8, u8)> {
    let Some(Value::Int(color_type)) = v.get("Type") else {
        return Err(err("FillColor.Type missing or not an integer"));
    };
    let Some(Value::List(values)) = v.get("Values") else {
        return Err(err("FillColor.Values missing or not a list"));
    };
    let as_f32 = |i: usize| -> Result<f32> {
        match values.get(i) {
            Some(Value::Float(f)) => Ok(*f as f32),
            Some(Value::Int(n)) => Ok(*n as f32),
            other => Err(err(format!(
                "FillColor.Values[{i}] missing or not numeric: {other:?}"
            ))),
        }
    };
    let to_u8 = |f: f32| -> u8 { (f.clamp(0.0, 1.0) * 255.0).round() as u8 };

    match color_type {
        1 => Ok((to_u8(as_f32(1)?), to_u8(as_f32(2)?), to_u8(as_f32(3)?))),
        0 => {
            let k = to_u8(as_f32(1)?);
            Ok((k, k, k))
        }
        2 => {
            let (c, m, y, k) = (as_f32(1)?, as_f32(2)?, as_f32(3)?, as_f32(4)?);
            Ok((
                to_u8((1.0 - c) * (1.0 - k)),
                to_u8((1.0 - m) * (1.0 - k)),
                to_u8((1.0 - y) * (1.0 - k)),
            ))
        }
        other => Err(err(format!("unsupported FillColor.Type {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `EngineData` payload actually embedded in a Photoshop-authored
    /// `TySh` block (extracted via `descriptor.rs` from `reference/tysh.bin`,
    /// which itself came from a real Photoshop-exported file — see
    /// `reference/README.md`). This is genuine raw wire-format bytes, unlike
    /// `reference/engineData.txt` — see the note on that file below.
    fn real_engine_data() -> Vec<u8> {
        engine_data_from_tysh(include_bytes!("../reference/tysh.bin"))
    }

    /// Same extraction as `real_engine_data`, generalized to any TySh
    /// fixture — used to reach the paragraph-text fixture's `EngineData`
    /// too (see `distinguishes_point_from_paragraph_text`).
    fn engine_data_from_tysh(tysh_bytes: &[u8]) -> Vec<u8> {
        let tysh = crate::descriptor::parse_fixture(tysh_bytes).expect("parse TySh");
        match tysh.text_data.get(b"EngineData") {
            Some(crate::descriptor::Value::Raw(bytes)) => bytes.clone(),
            other => panic!("expected EngineData to be Raw, got {other:?}"),
        }
    }

    /// `EngineDict.Rendered.Shapes.Children[0].Cookie.Photoshop.ShapeType`
    /// — 0 for point text, 1 for paragraph text. Not reachable by
    /// `Value::get_path` alone since it crosses a `List`, not just `Dict`s;
    /// confirmed against psd-tools' own `TypeLayer.text_type` property
    /// (`api/layers.py`) before trusting this path.
    fn shape_type(v: &Value) -> Option<i64> {
        let Value::List(children) = v.get_path("EngineDict.Rendered.Shapes.Children")? else {
            return None;
        };
        match children.first()?.get_path("Cookie.Photoshop.ShapeType")? {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }

    #[test]
    fn parses_a_real_photoshop_engine_data_payload() {
        let v = parse(&real_engine_data()).expect("a real EngineData payload must parse");
        assert_eq!(
            v.get_path("EngineDict.Editor.Text").and_then(Value::as_str),
            Some("Line 1\rLine 2\rLine 3 and text\r")
        );
        // Confirms the parser actually descended into deeply nested
        // structure, not just the first couple of keys.
        assert!(v.get_path("ResourceDict.KinsokuSet").is_some());
        assert!(v.get_path("EngineDict.ParagraphRun.RunArray").is_some());
    }

    #[test]
    fn distinguishes_point_from_paragraph_text() {
        // reference/tysh.bin (point text) vs. reference/tysh-paragraph.bin
        // (paragraph/area text, added to close the corpus gap named in
        // PLAN.md 0.6 / FINDINGS.md recommendation 4). Point-vs-paragraph
        // is not visible in the outer TySh descriptor at all — only here,
        // inside EngineData — so this is the one place that distinction
        // can actually be tested.
        let point = parse(&real_engine_data()).expect("parse point-text EngineData");
        let paragraph = parse(&engine_data_from_tysh(include_bytes!(
            "../reference/tysh-paragraph.bin"
        )))
        .expect("parse paragraph-text EngineData");

        assert_eq!(
            shape_type(&point),
            Some(0),
            "reference/tysh.bin should be point text"
        );
        assert_eq!(
            shape_type(&paragraph),
            Some(1),
            "reference/tysh-paragraph.bin should be paragraph text"
        );
    }

    #[test]
    fn reads_font_size_and_fill_color_from_the_real_fixture() {
        let v = parse(&real_engine_data()).expect("parse");
        let style = first_run_style(&v).expect("real fixture must have a first style run");
        assert_eq!(style.font_size, 13.0);
        assert_eq!(
            style.color,
            (0, 0, 0),
            "real fixture's FillColor is opaque black"
        );
    }

    #[test]
    fn decodes_fill_color_variants() {
        // RGB (Type 1): Values = [alpha_or_1, R, G, B].
        let rgb = Value::Dict(vec![
            ("Type".into(), Value::Int(1)),
            (
                "Values".into(),
                Value::List(vec![
                    Value::Float(1.0),
                    Value::Float(0.2),
                    Value::Float(0.4),
                    Value::Float(0.6),
                ]),
            ),
        ]);
        assert_eq!(decode_fill_color(&rgb).unwrap(), (51, 102, 153));

        // Grayscale (Type 0): Values = [1, K].
        let gray = Value::Dict(vec![
            ("Type".into(), Value::Int(0)),
            (
                "Values".into(),
                Value::List(vec![Value::Float(1.0), Value::Float(0.5)]),
            ),
        ]);
        assert_eq!(decode_fill_color(&gray).unwrap(), (128, 128, 128));

        // CMYK (Type 2): Values = [1, C, M, Y, K]. C=1,M=0,Y=1,K=0 -> green.
        let cmyk = Value::Dict(vec![
            ("Type".into(), Value::Int(2)),
            (
                "Values".into(),
                Value::List(vec![
                    Value::Float(1.0),
                    Value::Float(1.0),
                    Value::Float(0.0),
                    Value::Float(1.0),
                    Value::Float(0.0),
                ]),
            ),
        ]);
        assert_eq!(decode_fill_color(&cmyk).unwrap(), (0, 255, 0));
    }

    #[test]
    fn round_trips_and_reparses_identically() {
        let data = real_engine_data();
        let v = parse(&data).expect("parse");
        let out = write(&v);
        let reparsed = parse(&out).expect("re-parse of our own output");
        assert_eq!(reparsed, v, "semantic structure changed after round-trip");
    }

    #[test]
    fn patches_editor_text_and_stays_consistent() {
        let mut v = parse(&real_engine_data()).expect("parse");
        let slot = v
            .get_path_mut("EngineDict.Editor.Text")
            .expect("Editor.Text must exist in a real document");
        *slot = Value::Str("Aurora spike\r".into());

        let out = write(&v);
        let reparsed = parse(&out).expect("re-parse after patch");
        assert_eq!(
            reparsed
                .get_path("EngineDict.Editor.Text")
                .and_then(Value::as_str),
            Some("Aurora spike\r")
        );
        // Everything else must be untouched by the patch.
        assert_eq!(
            reparsed.get_path("ResourceDict.KinsokuSet"),
            v.get_path("ResourceDict.KinsokuSet")
        );
    }

    #[test]
    fn patches_text_and_recomputes_run_lengths() {
        // Before the fix (FINDINGS.md finding 10): the real fixture's
        // ParagraphRun.RunLengthArray is [7, 7, 16] (three paragraphs,
        // summing to the original 30-char text) and StyleRun.RunLengthArray
        // is [30] (one style run). Patching Editor.Text alone leaves both
        // stale — they'd still sum to 30 after the text became 13 chars,
        // which is exactly the internal inconsistency finding 10 flagged.
        let mut v = parse(&real_engine_data()).expect("parse");
        assert_eq!(
            v.get_path("EngineDict.ParagraphRun.RunLengthArray"),
            Some(&Value::List(vec![
                Value::Int(7),
                Value::Int(7),
                Value::Int(16)
            ]))
        );

        let new_text = "Aurora spike\r";
        *v.get_path_mut("EngineDict.Editor.Text")
            .expect("Editor.Text must exist") = Value::Str(new_text.into());
        recompute_run_lengths(&mut v, new_text.encode_utf16().count())
            .expect("a real fixture must have well-formed ParagraphRun/StyleRun");

        let new_len = Value::Int(new_text.encode_utf16().count() as i64);
        assert_eq!(
            v.get_path("EngineDict.ParagraphRun.RunLengthArray"),
            Some(&Value::List(vec![new_len.clone()]))
        );
        assert_eq!(
            v.get_path("EngineDict.StyleRun.RunLengthArray"),
            Some(&Value::List(vec![new_len]))
        );

        // Formatting is preserved, not discarded — the single remaining run
        // in each array is still the *first* original run's sheet, not a
        // blanked-out placeholder. `get_path` doesn't cross `List`s (see
        // `shape_type` above), so index into RunArray directly.
        let first_run = |run_key: &str| -> &Value {
            let Some(Value::List(run_array)) =
                v.get_path(&format!("EngineDict.{run_key}.RunArray"))
            else {
                panic!("{run_key}.RunArray missing or not a list");
            };
            run_array
                .first()
                .expect("RunArray must have one entry left")
        };
        assert_eq!(
            first_run("ParagraphRun").get_path("ParagraphSheet.Properties.Justification"),
            Some(&Value::Int(0))
        );
        assert_eq!(
            first_run("StyleRun").get_path("StyleSheet.StyleSheetData.FontSize"),
            Some(&Value::Float(13.0))
        );

        // Whole thing still round-trips after the combined patch.
        let out = write(&v);
        let reparsed = parse(&out).expect("re-parse after patch + recompute");
        assert_eq!(reparsed, v);
    }

    #[test]
    fn round_trips_unicode_whose_bytes_collide_with_delimiters() {
        // U+2913 has UTF-16BE bytes [0x29, 0x13] -- 0x29 is ')', the string
        // terminator this format uses. A unit-alignment-aware escaper (an
        // earlier version of write_string) would not escape it, because
        // 0x29 lands in the *high* byte of the unit, not the low byte where
        // ASCII '(' / ')' / '\' normally appear. That produces a literal,
        // unescaped ')' byte in the output, which read_string then
        // misreads as the string's own terminator -- silent truncation.
        //
        // Found and fixed *because* this test was written before trusting
        // the "obviously correct" first implementation, not after a bug
        // report -- consistent with this spike's established discipline of
        // checking against real bytes rather than assuming a parser that
        // compiles is a parser that's right.
        let tricky = format!(
            "before{}after\\with(parens)",
            char::from_u32(0x2913).unwrap()
        );
        let v = Value::Str(tricky.clone());
        let out = write(&v);
        let reparsed = parse(&out).expect("must parse despite delimiter-colliding bytes");
        assert_eq!(
            reparsed,
            Value::Str(tricky),
            "string was corrupted or truncated on round-trip"
        );
    }
}
