//! Schema meta-info for hover popovers and live JSON validation of request bodies.
//!
//! UI-free. The app's body editors ask:
//! * [`json_path_at`] — which JSON key / value is under a byte offset (tolerant of
//!   incomplete JSON while typing, and of `{{var}}` placeholders);
//! * [`field_info`] / [`param_info`] — what the linked OpenAPI operation says about
//!   that field or parameter (type, required, constraints, description, schema path);
//! * [`check_json`] — syntax check with `{{var}}` masking, the error mapped to a byte
//!   range and a Russian message;
//! * [`json_path_range`] — the reverse of `json_path_at`: where a schema mismatch path
//!   (`$.items[0].quantity`) lives in the original text;
//! * [`validate_request_body`] — schema mismatches of a request body.

use std::ops::Range;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::core::openapi::Spec;
use crate::core::schema;
use crate::core::types::{Mismatch, SpecRef};

/// A request linked to an operation of a loaded spec.
#[derive(Clone, Debug)]
pub struct SchemaLink {
    pub spec: Arc<Spec>,
    pub spec_ref: SpecRef,
}

impl SchemaLink {
    pub fn new(spec: Arc<Spec>, spec_ref: SpecRef) -> Self {
        Self { spec, spec_ref }
    }
}

impl PartialEq for SchemaLink {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.spec, &other.spec) && self.spec_ref == other.spec_ref
    }
}

/// One step of a JSON path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathSegment {
    Key(String),
    Index(usize),
}

/// Which body of the operation a path refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodySide {
    Request,
    Response(u16),
}

/// What the schema says about one field (or parameter).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FieldInfo {
    pub name: String,
    /// `integer`, `string`, `OrderItem[]`, `Address`…
    pub type_label: String,
    pub required: bool,
    /// `min: 1`, `enum: pending | paid`, `format: date-time`, `nullable`, `default: 20`…
    pub constraints: Vec<String>,
    pub description: Option<String>,
    /// `OrderItem.quantity`, or `requestBody.items[].quantity` without named models.
    pub schema_path: String,
    pub example: Option<String>,
    /// Parameters only: `query`, `header`, `path`, `cookie`.
    pub location: Option<String>,
}

/// `$.items[0].quantity` — the format of [`Mismatch::path`].
pub fn path_to_string(path: &[PathSegment]) -> String {
    let mut out = String::from("$");
    for seg in path {
        match seg {
            PathSegment::Key(k) => {
                let ident = !k.is_empty()
                    && k.chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '$' || c == '-')
                    && !k.chars().next().is_some_and(|c| c.is_ascii_digit());
                if ident {
                    out.push('.');
                    out.push_str(k);
                } else {
                    out.push('[');
                    out.push_str(&Value::String(k.clone()).to_string());
                    out.push(']');
                }
            }
            PathSegment::Index(i) => out.push_str(&format!("[{i}]")),
        }
    }
    out
}

/// Parses a [`Mismatch::path`] (`$.a[0]["b.c"]`) back into segments.
pub fn parse_path(path: &str) -> Option<Vec<PathSegment>> {
    let rest = path.strip_prefix('$')?;
    let chars: Vec<char> = rest.chars().collect();
    let mut i = 0;
    let mut segs = Vec::new();
    while i < chars.len() {
        match chars[i] {
            '.' => {
                let start = i + 1;
                i = start;
                while i < chars.len() && chars[i] != '.' && chars[i] != '[' {
                    i += 1;
                }
                segs.push(PathSegment::Key(chars[start..i].iter().collect()));
            }
            '[' if chars.get(i + 1) == Some(&'"') => {
                let mut j = i + 2;
                while j < chars.len() && chars[j] != '"' {
                    if chars[j] == '\\' {
                        j += 1;
                    }
                    j += 1;
                }
                let lit: String = chars.get(i + 1..=j)?.iter().collect();
                segs.push(PathSegment::Key(serde_json::from_str(&lit).ok()?));
                if chars.get(j + 1) != Some(&']') {
                    return None;
                }
                i = j + 2;
            }
            '[' => {
                let end = chars[i..].iter().position(|&c| c == ']')? + i;
                let idx = chars[i + 1..end].iter().collect::<String>().parse().ok()?;
                segs.push(PathSegment::Index(idx));
                i = end + 1;
            }
            _ => return None,
        }
    }
    Some(segs)
}

// ---------------------------------------------------------------------------
// Tolerant JSON scanner

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TokenKind {
    /// An object key (string, quotes included in the range).
    Key,
    /// A scalar value, a `{{var}}` placeholder, or the opening bracket of a container.
    Value,
}

enum Frame {
    Object { key: Option<String>, expect_key: bool },
    Array { index: usize },
}

fn current_path(stack: &[Frame]) -> Vec<PathSegment> {
    let mut path = Vec::new();
    for frame in stack {
        match frame {
            Frame::Object {
                key: Some(k),
                expect_key: false,
            } => path.push(PathSegment::Key(k.clone())),
            Frame::Object { .. } => {}
            Frame::Array { index } => path.push(PathSegment::Index(*index)),
        }
    }
    path
}

/// End of a string literal starting at `start` (the opening quote). Unterminated
/// strings stop at the line break.
fn string_end(bytes: &[u8], start: usize) -> (usize, bool) {
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return (i + 1, true),
            b'\n' => return (i, false),
            _ => i += 1,
        }
    }
    (bytes.len(), false)
}

/// Walks every key / value token with its JSON path. `f` returns `true` to stop.
fn scan(text: &str, mut f: impl FnMut(TokenKind, Range<usize>, &[PathSegment]) -> bool) {
    let bytes = text.as_bytes();
    let mut stack: Vec<Frame> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'{' if bytes.get(i + 1) == Some(&b'{') => {
                // `{{var}}` placeholder used as a value.
                let end = text[i..]
                    .find("}}")
                    .map(|e| i + e + 2)
                    .filter(|&e| !text[i..e].contains('\n'))
                    .unwrap_or(i + 2);
                let path = current_path(&stack);
                if f(TokenKind::Value, i..end, &path) {
                    return;
                }
                i = end;
            }
            b'{' | b'[' => {
                let path = current_path(&stack);
                if f(TokenKind::Value, i..i + 1, &path) {
                    return;
                }
                stack.push(if b == b'{' {
                    Frame::Object {
                        key: None,
                        expect_key: true,
                    }
                } else {
                    Frame::Array { index: 0 }
                });
                i += 1;
            }
            b'}' | b']' => {
                stack.pop();
                i += 1;
            }
            b':' => {
                if let Some(Frame::Object { expect_key, .. }) = stack.last_mut() {
                    *expect_key = false;
                }
                i += 1;
            }
            b',' => {
                match stack.last_mut() {
                    Some(Frame::Object { key, expect_key }) => {
                        *key = None;
                        *expect_key = true;
                    }
                    Some(Frame::Array { index }) => *index += 1,
                    None => {}
                }
                i += 1;
            }
            b'"' => {
                let (end, closed) = string_end(bytes, i);
                let is_key = matches!(
                    stack.last(),
                    Some(Frame::Object {
                        expect_key: true,
                        ..
                    })
                );
                if is_key {
                    let raw = &text[i..end];
                    let key = if closed {
                        serde_json::from_str::<String>(raw)
                            .unwrap_or_else(|_| raw[1..raw.len() - 1].to_string())
                    } else {
                        raw[1..].to_string()
                    };
                    let mut path = current_path(&stack);
                    path.push(PathSegment::Key(key.clone()));
                    if f(TokenKind::Key, i..end, &path) {
                        return;
                    }
                    if let Some(Frame::Object { key: slot, .. }) = stack.last_mut() {
                        *slot = Some(key);
                    }
                } else {
                    let path = current_path(&stack);
                    if f(TokenKind::Value, i..end, &path) {
                        return;
                    }
                }
                i = end;
            }
            _ => {
                // Number / literal / garbage: a run of non-structural characters.
                let start = i;
                while i < bytes.len()
                    && !matches!(
                        bytes[i],
                        b' ' | b'\t' | b'\r' | b'\n' | b'{' | b'}' | b'[' | b']' | b':' | b','
                            | b'"'
                    )
                {
                    i += 1;
                }
                let path = current_path(&stack);
                if f(TokenKind::Value, start..i, &path) {
                    return;
                }
            }
        }
    }
}

/// Path of the key or value under byte `offset` (inclusive of the token end, so a
/// pointer on the right half of a closing quote still counts). `None` on whitespace,
/// punctuation or outside any token.
pub fn json_path_at(text: &str, offset: usize) -> Option<Vec<PathSegment>> {
    json_token_at(text, offset).map(|(_, path)| path)
}

/// Like [`json_path_at`], plus the byte range of the token under `offset`.
pub fn json_token_at(text: &str, offset: usize) -> Option<(Range<usize>, Vec<PathSegment>)> {
    let mut found = None;
    scan(text, |_, range, path| {
        if range.start > offset {
            return true;
        }
        if offset < range.end || (offset == range.end && range.len() > 1) {
            found = Some((range, path.to_vec()));
            return true;
        }
        false
    });
    found
}

/// Byte range of the key token (for an object member) or of the value (array
/// element, root) at `path` in the original text. Falls back to the longest
/// existing prefix (a missing required property points at its parent).
pub fn json_path_range(text: &str, path: &[PathSegment]) -> Option<Range<usize>> {
    for len in (0..=path.len()).rev() {
        let target = &path[..len];
        let mut found = None;
        scan(text, |_, range, p| {
            if p == target {
                found = Some(range);
                return true;
            }
            false
        });
        if found.is_some() {
            return found;
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Syntax check

/// A JSON syntax error mapped onto the original text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonSyntaxError {
    /// Byte range of the offending token (at least one character).
    pub range: Range<usize>,
    /// Russian message with location, e.g. ``Ожидается `,` или `}` (стр. 4, кол. 12)``.
    pub message: String,
    /// 1-based line / column as reported by the parser.
    pub line: usize,
    pub column: usize,
}

/// Replaces every `{{…}}` with a same-length JSON-valid stand-in so byte offsets
/// stay aligned: `x`s inside strings, `0` padded with spaces outside.
pub fn mask_placeholders(text: &str) -> String {
    if !text.contains("{{") {
        return text.to_string();
    }
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'{'
            && bytes.get(i + 1) == Some(&b'{')
            && let Some(rel) = text[i + 2..].find("}}")
        {
            let end = i + 2 + rel + 2;
            let inner = &text[i..end];
            if !inner.contains('\n') && !inner[2..inner.len() - 2].contains('{') {
                if in_string {
                    out.extend(std::iter::repeat_n(b'x', end - i));
                } else {
                    out.push(b'0');
                    out.extend(std::iter::repeat_n(b' ', end - i - 1));
                }
                i = end;
                continue;
            }
        }
        if in_string {
            if b == b'\\' {
                out.push(b);
                if let Some(&next) = bytes.get(i + 1) {
                    out.push(next);
                }
                i += 2;
                continue;
            }
            if b == b'"' || b == b'\n' {
                in_string = false;
            }
        } else if b == b'"' {
            in_string = true;
        }
        out.push(b);
        i += 1;
    }
    // Only ASCII bytes were substituted for whole placeholders, so this stays UTF-8.
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

fn translate_error(msg: &str) -> String {
    let ru = match msg {
        "EOF while parsing a value" => "Неожиданный конец JSON: ожидается значение",
        "EOF while parsing an object" => "Неожиданный конец JSON: объект не закрыт `}`",
        "EOF while parsing a list" => "Неожиданный конец JSON: массив не закрыт `]`",
        "EOF while parsing a string" => "Неожиданный конец JSON: строка не закрыта `\"`",
        "expected `,` or `}`" => "Ожидается `,` или `}`",
        "expected `,` or `]`" => "Ожидается `,` или `]`",
        "expected `:`" => "Ожидается `:`",
        "expected value" => "Ожидается значение",
        "expected ident" => "Неизвестный литерал: ожидается `true`, `false` или `null`",
        "trailing comma" => "Лишняя запятая",
        "key must be a string" => "Ключ должен быть строкой в двойных кавычках",
        "invalid escape" => "Некорректная escape-последовательность",
        "invalid number" => "Некорректное число",
        "number out of range" => "Число вне допустимого диапазона",
        "invalid unicode code point" => "Некорректный код Unicode",
        "trailing characters" => "Лишние символы после JSON",
        "control character (\\u0000-\\u001F) found while parsing a string" => {
            "Управляющий символ внутри строки"
        }
        "recursion limit exceeded" => "Слишком глубокая вложенность",
        "lone leading surrogate in hex escape" | "unexpected end of hex escape" => {
            "Некорректная escape-последовательность \\u"
        }
        "EOF while parsing" => "Неожиданный конец JSON",
        _ => return msg.to_string(),
    };
    ru.to_string()
}

fn line_start(text: &str, line: usize) -> usize {
    if line <= 1 {
        return 0;
    }
    text.match_indices('\n')
        .nth(line - 2)
        .map(|(i, _)| i + 1)
        .unwrap_or(text.len())
}

fn floor_char(text: &str, mut i: usize) -> usize {
    i = i.min(text.len());
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn next_char_end(text: &str, i: usize) -> usize {
    text[i..]
        .chars()
        .next()
        .map(|c| i + c.len_utf8())
        .unwrap_or(text.len())
}

/// Range of the token starting at `at`: a whole string / word, else one character.
fn token_range(text: &str, at: usize) -> Range<usize> {
    let bytes = text.as_bytes();
    match bytes.get(at) {
        Some(b'"') => at..string_end(bytes, at).0.max(at + 1),
        Some(b) if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'+' | b'.' | b'_') => {
            let mut end = at;
            while end < bytes.len()
                && (bytes[end].is_ascii_alphanumeric()
                    || matches!(bytes[end], b'-' | b'+' | b'.' | b'_'))
            {
                end += 1;
            }
            at..end
        }
        _ => at..next_char_end(text, at),
    }
}

fn last_non_ws(text: &str) -> Option<Range<usize>> {
    let (i, c) = text.char_indices().rev().find(|(_, c)| !c.is_whitespace())?;
    Some(i..i + c.len_utf8())
}

/// Parses `text` as JSON (after [`mask_placeholders`]). An empty / blank body is
/// `Ok(None)`.
pub fn check_json(text: &str) -> Result<Option<Value>, JsonSyntaxError> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let masked = mask_placeholders(text);
    let err = match serde_json::from_str::<Value>(&masked) {
        Ok(v) => return Ok(Some(v)),
        Err(e) => e,
    };
    let full = err.to_string();
    let msg = full
        .rsplit_once(" at line ")
        .map(|(m, _)| m.to_string())
        .unwrap_or(full);
    let (line, column) = (err.line(), err.column());
    let range = if err.is_eof() {
        last_non_ws(text).unwrap_or(0..text.len().min(1))
    } else {
        let at = floor_char(
            text,
            line_start(text, line) + column.saturating_sub(1),
        );
        if msg == "trailing comma" {
            // Point at the comma itself, not at the closing bracket after it.
            match text[..at].rfind(',') {
                Some(c) => c..c + 1,
                None => token_range(text, at),
            }
        } else if at >= text.len() {
            last_non_ws(text).unwrap_or(0..text.len().min(1))
        } else {
            token_range(text, at)
        }
    };
    let range = if range.is_empty() {
        let s = floor_char(text, range.start.min(text.len().saturating_sub(1)));
        s..next_char_end(text, s)
    } else {
        range
    };
    Err(JsonSyntaxError {
        message: format!("{} (стр. {line}, кол. {column})", translate_error(&msg)),
        range,
        line,
        column,
    })
}

// ---------------------------------------------------------------------------
// Spec lookups

fn template_matches(template: &str, path: &str) -> bool {
    if !template.contains('{') {
        return false;
    }
    let t: Vec<&str> = template.trim_end_matches('/').split('/').collect();
    let p: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    t.len() == p.len()
        && t.iter().zip(&p).all(|(a, b)| {
            if a.starts_with('{') && a.ends_with('}') {
                !b.is_empty()
            } else {
                a == b
            }
        })
}

/// (path item, operation) of a spec ref.
fn operation<'a>(spec: &'a Spec, r: &SpecRef) -> Option<(&'a Value, &'a Value)> {
    let doc = spec.document();
    let paths = doc.get("paths")?.as_object()?;
    let path = r.path.split(['?', '#']).next().unwrap_or(&r.path);
    let item = paths.get(path).or_else(|| {
        paths
            .iter()
            .find(|(t, _)| template_matches(t, path))
            .map(|(_, v)| v)
    })?;
    let item = schema::deref(doc, item);
    let op = item
        .get(r.method.label().to_ascii_lowercase().as_str())
        .filter(|v| v.is_object())?;
    Some((item, op))
}

fn json_media(content: &Value) -> Option<&Value> {
    let map = content.as_object()?;
    map.get("application/json")
        .or_else(|| map.iter().find(|(k, _)| k.contains("json")).map(|(_, v)| v))
        .or_else(|| map.get("*/*"))
}

fn swagger2(spec: &Spec) -> bool {
    spec.document().get("swagger").is_some()
}

/// JSON schema of the operation's request body.
pub fn request_body_schema<'a>(spec: &'a Spec, r: &SpecRef) -> Option<&'a Value> {
    let doc = spec.document();
    let (item, op) = operation(spec, r)?;
    if swagger2(spec) {
        return item
            .get("parameters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .chain(op.get("parameters").and_then(Value::as_array).into_iter().flatten())
            .map(|p| schema::deref(doc, p))
            .find(|p| p.get("in").and_then(Value::as_str) == Some("body"))?
            .get("schema");
    }
    let body = schema::deref(doc, op.get("requestBody")?);
    json_media(body.get("content")?)?.get("schema")
}

/// (matched response key, schema) for a status code: exact, `2XX`, `default`.
pub fn response_body_schema<'a>(
    spec: &'a Spec,
    r: &SpecRef,
    status: u16,
) -> Option<(String, &'a Value)> {
    let doc = spec.document();
    let responses = operation(spec, r)?.1.get("responses")?.as_object()?;
    let exact = status.to_string();
    let range = format!("{}XX", status / 100);
    let key = responses
        .keys()
        .find(|k| **k == exact)
        .or_else(|| responses.keys().find(|k| k.eq_ignore_ascii_case(&range)))
        .or_else(|| responses.keys().find(|k| *k == "default"))?;
    let response = schema::deref(doc, &responses[key]);
    let schema = if swagger2(spec) {
        response.get("schema")?
    } else {
        json_media(response.get("content")?)?.get("schema")?
    };
    Some((key.clone(), schema))
}

/// Schema mismatches of a request body; `None` when the operation has no JSON schema.
pub fn validate_request_body(spec: &Spec, r: &SpecRef, body: &Value) -> Option<Vec<Mismatch>> {
    let schema = request_body_schema(spec, r)?;
    Some(schema::validate(spec.document(), schema, body))
}

// ---------------------------------------------------------------------------
// Schema walking

const MAX_HOPS: usize = 32;

/// Follows `$ref`s, remembering the last named model.
fn resolve<'a>(doc: &'a Value, mut s: &'a Value, model: &mut Option<String>) -> &'a Value {
    for _ in 0..MAX_HOPS {
        let Some(r) = s.get("$ref").and_then(Value::as_str) else {
            break;
        };
        match schema::resolve_ref(doc, r) {
            Some(next) => {
                *model = Some(schema::ref_name(r).to_string());
                s = next;
            }
            None => break,
        }
    }
    s
}

/// Property `key` of an object schema: own `properties`, then `allOf` parts, then the
/// first `oneOf`/`anyOf` branch that declares it, then `additionalProperties`.
/// Returns (property schema, required, model owning the property).
fn find_property<'a>(
    doc: &'a Value,
    s: &'a Value,
    key: &str,
    model: &Option<String>,
    depth: usize,
) -> Option<(&'a Value, bool, Option<String>)> {
    if depth > MAX_HOPS {
        return None;
    }
    let obj = s.as_object()?;
    let required_here = |o: &Map<String, Value>| {
        o.get("required")
            .and_then(Value::as_array)
            .is_some_and(|r| r.iter().any(|v| v.as_str() == Some(key)))
    };
    if let Some(prop) = obj
        .get("properties")
        .and_then(|p| p.get(key))
    {
        return Some((prop, required_here(obj), model.clone()));
    }
    if let Some(parts) = obj.get("allOf").and_then(Value::as_array) {
        for part in parts {
            let mut m = model.clone();
            let resolved = resolve(doc, part, &mut m);
            if let Some((prop, req, owner)) = find_property(doc, resolved, key, &m, depth + 1) {
                // `required` may be declared on a sibling part or on the allOf owner.
                let req = req
                    || required_here(obj)
                    || parts.iter().any(|p| {
                        resolve(doc, p, &mut None)
                            .as_object()
                            .is_some_and(required_here)
                    });
                return Some((prop, req, owner));
            }
        }
    }
    for k in ["oneOf", "anyOf"] {
        if let Some(branches) = obj.get(k).and_then(Value::as_array) {
            for branch in branches {
                let mut m = model.clone();
                let resolved = resolve(doc, branch, &mut m);
                if let Some(found) = find_property(doc, resolved, key, &m, depth + 1) {
                    return Some(found);
                }
            }
        }
    }
    match obj.get("additionalProperties") {
        Some(ap) if ap.is_object() => Some((ap, required_here(obj), model.clone())),
        _ => None,
    }
}

/// `items` of an array schema (also through `allOf`/`oneOf`/`anyOf`).
fn find_items<'a>(doc: &'a Value, s: &'a Value, depth: usize) -> Option<&'a Value> {
    if depth > MAX_HOPS {
        return None;
    }
    let obj = s.as_object()?;
    if let Some(items) = obj.get("items") {
        return Some(items);
    }
    for k in ["allOf", "oneOf", "anyOf"] {
        if let Some(parts) = obj.get(k).and_then(Value::as_array) {
            for part in parts {
                if let Some(items) = find_items(doc, schema::deref(doc, part), depth + 1) {
                    return Some(items);
                }
            }
        }
    }
    None
}

/// First value of `key` on the schema or its `allOf` parts.
fn lookup<'a>(doc: &'a Value, s: &'a Value, key: &str, depth: usize) -> Option<&'a Value> {
    if depth > 8 {
        return None;
    }
    let s = schema::deref(doc, s);
    if let Some(v) = s.get(key) {
        return Some(v);
    }
    s.get("allOf")
        .and_then(Value::as_array)?
        .iter()
        .find_map(|p| lookup(doc, p, key, depth + 1))
}

fn scalar_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn types_of(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

/// Short type label: `integer`, `string`, `OrderItem[]`, `Address`, `object`, `any`.
fn type_label(doc: &Value, s: &Value, depth: usize) -> String {
    if depth > 4 {
        return "any".into();
    }
    if let Some(r) = s.get("$ref").and_then(Value::as_str) {
        let target = schema::deref(doc, s);
        let ty = type_label(doc, target, depth + 1);
        // Named objects read better by name; named scalars (enums) by their type.
        return if ty == "object" || ty == "any" {
            schema::ref_name(r).to_string()
        } else {
            ty
        };
    }
    let mut types: Vec<String> = lookup(doc, s, "type", 0).map(types_of).unwrap_or_default();
    types.retain(|t| t != "null");
    let ty = match types.first().map(String::as_str) {
        Some(_) if types.len() > 1 => types.join(" | "),
        Some(t) => t.to_string(),
        None if lookup(doc, s, "properties", 0).is_some() => "object".into(),
        None if s.get("items").is_some() => "array".into(),
        None => {
            if let Some(branches) = s
                .get("oneOf")
                .or_else(|| s.get("anyOf"))
                .and_then(Value::as_array)
            {
                return branches
                    .iter()
                    .map(|b| type_label(doc, b, depth + 1))
                    .collect::<Vec<_>>()
                    .join(" | ");
            }
            if let Some(v) = s.get("enum").and_then(Value::as_array).and_then(|a| a.first()) {
                schema::type_name(v).to_string()
            } else {
                "any".into()
            }
        }
    };
    if ty == "array"
        && let Some(items) = find_items(doc, s, 0)
    {
        return format!("{}[]", type_label(doc, items, depth + 1));
    }
    ty
}

fn fmt_num(v: &Value) -> String {
    match v.as_f64() {
        Some(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", f as i64),
        _ => scalar_text(v),
    }
}

fn constraints(doc: &Value, s: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let get = |k: &str| lookup(doc, s, k, 0);
    if let Some(values) = get("enum").and_then(Value::as_array) {
        let list: Vec<String> = values.iter().map(scalar_text).collect();
        out.push(format!("enum: {}", list.join(" | ")));
    }
    if let Some(c) = get("const") {
        out.push(format!("const: {}", scalar_text(c)));
    }
    let excl_min = get("exclusiveMinimum");
    let excl_max = get("exclusiveMaximum");
    if let Some(min) = get("minimum") {
        if excl_min == Some(&Value::Bool(true)) {
            out.push(format!("min: >{}", fmt_num(min)));
        } else {
            out.push(format!("min: {}", fmt_num(min)));
        }
    } else if let Some(m) = excl_min.filter(|v| v.is_number()) {
        out.push(format!("min: >{}", fmt_num(m)));
    }
    if let Some(max) = get("maximum") {
        if excl_max == Some(&Value::Bool(true)) {
            out.push(format!("max: <{}", fmt_num(max)));
        } else {
            out.push(format!("max: {}", fmt_num(max)));
        }
    } else if let Some(m) = excl_max.filter(|v| v.is_number()) {
        out.push(format!("max: <{}", fmt_num(m)));
    }
    if let Some(m) = get("multipleOf") {
        out.push(format!("multipleOf: {}", fmt_num(m)));
    }
    for (key, label) in [
        ("minLength", "minLength"),
        ("maxLength", "maxLength"),
        ("minItems", "minItems"),
        ("maxItems", "maxItems"),
        ("minProperties", "minProperties"),
        ("maxProperties", "maxProperties"),
    ] {
        if let Some(v) = get(key) {
            out.push(format!("{label}: {}", fmt_num(v)));
        }
    }
    if get("uniqueItems") == Some(&Value::Bool(true)) {
        out.push("uniqueItems".into());
    }
    if let Some(f) = get("format").and_then(Value::as_str) {
        out.push(format!("format: {f}"));
    }
    if let Some(p) = get("pattern").and_then(Value::as_str) {
        out.push(format!("pattern: {p}"));
    }
    let nullable = get("nullable") == Some(&Value::Bool(true))
        || get("type").is_some_and(|t| types_of(t).iter().any(|t| t == "null"));
    if nullable {
        out.push("nullable".into());
    }
    if let Some(d) = get("default") {
        out.push(format!("default: {}", scalar_text(d)));
    }
    if get("readOnly") == Some(&Value::Bool(true)) {
        out.push("readOnly".into());
    }
    if get("writeOnly") == Some(&Value::Bool(true)) {
        out.push("writeOnly".into());
    }
    if get("deprecated") == Some(&Value::Bool(true)) {
        out.push("deprecated".into());
    }
    out
}

fn text_of(doc: &Value, own: &Value, key: &str) -> Option<String> {
    own.get(key)
        .or_else(|| lookup(doc, own, key, 0))
        .map(scalar_text)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn example_of(doc: &Value, own: &Value) -> Option<String> {
    own.get("example")
        .or_else(|| lookup(doc, own, "example", 0))
        .map(|v| match v {
            Value::String(s) => s.clone(),
            other => serde_json::to_string(other).unwrap_or_default(),
        })
}

/// Schema meta-info for the field at `path` of the operation's request or response body.
pub fn field_info(
    spec: &Spec,
    spec_ref: &SpecRef,
    body: BodySide,
    path: &[PathSegment],
) -> Option<FieldInfo> {
    let doc = spec.document();
    let (root, root_label) = match body {
        BodySide::Request => (request_body_schema(spec, spec_ref)?, "requestBody".to_string()),
        BodySide::Response(status) => {
            let (key, s) = response_body_schema(spec, spec_ref, status)?;
            (s, format!("responses.{key}"))
        }
    };
    let last_key = path.iter().rposition(|s| matches!(s, PathSegment::Key(_)))?;
    let PathSegment::Key(field_name) = &path[last_key] else {
        return None;
    };

    let mut model: Option<String> = None;
    let mut rel = String::new();
    let mut current = root;
    let mut required = false;
    for seg in path {
        let before = model.clone();
        let resolved = resolve(doc, current, &mut model);
        if model != before {
            rel.clear();
        }
        match seg {
            PathSegment::Key(k) => {
                let (prop, req, owner) = find_property(doc, resolved, k, &model, 0)?;
                if owner != model {
                    model = owner;
                    rel.clear();
                }
                required = req;
                if !rel.is_empty() {
                    rel.push('.');
                }
                rel.push_str(k);
                current = prop;
            }
            PathSegment::Index(_) => {
                current = find_items(doc, resolved, 0)?;
                required = false;
                rel.push_str("[]");
            }
        }
    }

    let trailing = path.len() - last_key - 1;
    let name = format!("{field_name}{}", "[]".repeat(trailing));
    let schema_path = match &model {
        Some(m) => format!("{m}.{rel}"),
        None => format!("{root_label}.{rel}"),
    };
    Some(FieldInfo {
        name,
        type_label: type_label(doc, current, 0),
        required: required && trailing == 0,
        constraints: constraints(doc, current),
        description: text_of(doc, current, "description")
            .or_else(|| text_of(doc, current, "title")),
        schema_path,
        example: example_of(doc, current),
        location: None,
    })
}

/// Meta-info of a parameter (`location`: `query`, `header`, `path`, `cookie`).
/// Header names match case-insensitively.
pub fn param_info(spec: &Spec, spec_ref: &SpecRef, name: &str, location: &str) -> Option<FieldInfo> {
    let doc = spec.document();
    let (item, op) = operation(spec, spec_ref)?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let matches = |p: &Value| {
        p.get("in").and_then(Value::as_str) == Some(location)
            && p.get("name").and_then(Value::as_str).is_some_and(|n| {
                if location == "header" {
                    n.eq_ignore_ascii_case(name)
                } else {
                    n == name
                }
            })
    };
    // Operation-level parameters override path-item ones.
    let raw = op
        .get("parameters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(item.get("parameters").and_then(Value::as_array).into_iter().flatten())
        .find(|p| matches(schema::deref(doc, p)))?;
    let param = schema::deref(doc, raw);
    // OpenAPI 3: `schema`; Swagger 2: type keywords on the parameter itself.
    let s = param.get("schema").unwrap_or(param);
    let declared = param.get("name").and_then(Value::as_str).unwrap_or(name);
    let schema_path = match raw.get("$ref").and_then(Value::as_str) {
        Some(r) => format!("parameters.{}", schema::ref_name(r)),
        None => format!("{location}.{declared}"),
    };
    Some(FieldInfo {
        name: declared.to_string(),
        type_label: type_label(doc, s, 0),
        required: param.get("required").and_then(Value::as_bool).unwrap_or(false)
            || location == "path",
        constraints: constraints(doc, s),
        description: text_of(doc, param, "description").or_else(|| text_of(doc, s, "description")),
        schema_path,
        example: param
            .get("example")
            .map(scalar_text)
            .or_else(|| example_of(doc, s)),
        location: Some(location.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Method;

    fn k(s: &str) -> PathSegment {
        PathSegment::Key(s.into())
    }

    fn at(text: &str, needle: &str, delta: usize) -> usize {
        text.find(needle).unwrap() + delta
    }

    fn shop() -> Spec {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/shop-api.yaml"
        ))
        .unwrap();
        Spec::parse(&text).unwrap()
    }

    fn post_orders() -> SpecRef {
        SpecRef {
            method: Method::Post,
            path: "/orders".into(),
        }
    }

    #[test]
    fn path_on_nested_key_and_value() {
        let text = r#"{"customerId": "c1", "items": [{"productId": "p", "quantity": 2}]}"#;
        let q = at(text, "\"quantity\"", 3);
        assert_eq!(
            json_path_at(text, q),
            Some(vec![k("items"), PathSegment::Index(0), k("quantity")])
        );
        let v = at(text, "2}", 0);
        assert_eq!(
            json_path_at(text, v),
            Some(vec![k("items"), PathSegment::Index(0), k("quantity")])
        );
        assert_eq!(json_path_at(text, at(text, "\"c1\"", 1)), Some(vec![k("customerId")]));
        // Whitespace and punctuation are nothing.
        assert_eq!(json_path_at(text, at(text, ": \"c1\"", 1)), None);
        assert_eq!(json_path_at(text, at(text, ": \"c1\"", 0)), Some(vec![k("customerId")]));
        // The opening bracket of an array element is that element.
        assert_eq!(
            json_path_at(text, at(text, "{\"productId", 0)),
            Some(vec![k("items"), PathSegment::Index(0)])
        );
    }

    #[test]
    fn path_in_arrays() {
        let text = "[1, [2, 3], {\"a\": [true, null]}]";
        assert_eq!(json_path_at(text, at(text, "3", 0)), Some(vec![PathSegment::Index(1), PathSegment::Index(1)]));
        assert_eq!(
            json_path_at(text, at(text, "null", 2)),
            Some(vec![PathSegment::Index(2), k("a"), PathSegment::Index(1)])
        );
        assert_eq!(json_path_at(text, 0), Some(vec![]));
    }

    #[test]
    fn path_in_incomplete_json() {
        let text = "{\n  \"items\": [\n    {\"quantity\": ";
        assert_eq!(
            json_path_at(text, at(text, "quantity", 0)),
            Some(vec![k("items"), PathSegment::Index(0), k("quantity")])
        );
        // Unterminated key while typing.
        let text = "{\"customerId\": \"x\", \"ite";
        assert_eq!(json_path_at(text, text.len() - 1), Some(vec![k("ite")]));
        // Placeholders are values.
        let text = "{\"n\": {{count}}, \"m\": 1}";
        assert_eq!(json_path_at(text, at(text, "count", 0)), Some(vec![k("n")]));
        assert_eq!(json_path_at(text, at(text, "\"m\"", 1)), Some(vec![k("m")]));
    }

    #[test]
    fn path_round_trips_through_strings_and_ranges() {
        let path = vec![k("items"), PathSegment::Index(0), k("a.b")];
        assert_eq!(path_to_string(&path), "$.items[0][\"a.b\"]");
        assert_eq!(parse_path(&path_to_string(&path)), Some(path));

        let text = "{\n  \"items\": [\n    {\"quantity\": \"2\"}\n  ]\n}";
        let r = json_path_range(text, &parse_path("$.items[0].quantity").unwrap()).unwrap();
        assert_eq!(&text[r], "\"quantity\"");
        // Missing field → its parent object.
        let r = json_path_range(text, &parse_path("$.items[0].productId").unwrap()).unwrap();
        assert_eq!(&text[r], "{");
        let r = json_path_range(text, &[]).unwrap();
        assert_eq!(r, 0..1);
    }

    #[test]
    fn masks_placeholders_keeping_offsets() {
        let text = r#"{"customerId": "{{customerId}}", "n": {{count}}}"#;
        let masked = mask_placeholders(text);
        assert_eq!(masked.len(), text.len());
        assert!(serde_json::from_str::<Value>(&masked).is_ok(), "{masked}");
        assert_eq!(check_json(text).unwrap().unwrap()["n"], 0);
        assert!(masked.contains("\"xxxxxxxxxxxxxx\""));
        assert_eq!(check_json("  \n").unwrap(), None);
    }

    #[test]
    fn maps_syntax_errors_to_ranges() {
        let text = "{\n  \"a\": 1\n  \"b\": 2\n}";
        let e = check_json(text).unwrap_err();
        assert_eq!(&text[e.range.clone()], "\"b\"");
        assert!(e.message.starts_with("Ожидается `,` или `}`"), "{}", e.message);
        assert!(e.message.ends_with("(стр. 3, кол. 3)"), "{}", e.message);

        let text = "{\"a\": 1,}";
        let e = check_json(text).unwrap_err();
        assert_eq!(&text[e.range.clone()], ",");
        assert!(e.message.starts_with("Лишняя запятая"));

        let text = "{\"a\": [1, 2";
        let e = check_json(text).unwrap_err();
        assert_eq!(&text[e.range.clone()], "2");
        assert!(e.message.starts_with("Неожиданный конец JSON"));

        let text = "{a: 1}";
        let e = check_json(text).unwrap_err();
        assert_eq!(e.range, 1..2);
        assert!(e.message.starts_with("Ключ должен быть строкой"));

        let text = "{\"a\": tru}";
        let e = check_json(text).unwrap_err();
        assert!(!e.range.is_empty());
    }

    #[test]
    fn request_field_info_from_shop_api() {
        let spec = shop();
        let info = field_info(
            &spec,
            &post_orders(),
            BodySide::Request,
            &[k("items"), PathSegment::Index(0), k("quantity")],
        )
        .unwrap();
        assert_eq!(info.name, "quantity");
        assert_eq!(info.type_label, "integer");
        assert!(info.required);
        assert_eq!(info.constraints, vec!["min: 1".to_string()]);
        assert_eq!(
            info.description.as_deref(),
            Some("Количество единиц товара в позиции заказа")
        );
        assert_eq!(info.schema_path, "OrderItem.quantity");
        assert_eq!(info.example.as_deref(), Some("2"));

        let items = field_info(&spec, &post_orders(), BodySide::Request, &[k("items")]).unwrap();
        assert_eq!(items.type_label, "OrderItem[]");
        assert!(items.required);
        assert_eq!(items.schema_path, "OrderCreate.items");
        assert_eq!(items.constraints, vec!["minItems: 1".to_string()]);

        let price = field_info(
            &spec,
            &post_orders(),
            BodySide::Request,
            &[k("items"), PathSegment::Index(0), k("price")],
        )
        .unwrap();
        assert!(!price.required);
        assert!(field_info(&spec, &post_orders(), BodySide::Request, &[k("nope")]).is_none());
    }

    #[test]
    fn response_field_info_and_inline_fallback() {
        let spec = shop();
        let status =
            field_info(&spec, &post_orders(), BodySide::Response(201), &[k("status")]).unwrap();
        assert_eq!(status.type_label, "string");
        assert!(status.required);
        assert_eq!(status.schema_path, "Order.status");
        assert_eq!(
            status.constraints,
            vec!["enum: pending | paid | shipped | cancelled".to_string()]
        );
        let created =
            field_info(&spec, &post_orders(), BodySide::Response(201), &[k("createdAt")]).unwrap();
        assert!(created.constraints.contains(&"format: date-time".to_string()));

        // Inline schema without a named model.
        let login = SpecRef {
            method: Method::Post,
            path: "/auth/login".into(),
        };
        let pw = field_info(&spec, &login, BodySide::Request, &[k("password")]).unwrap();
        assert_eq!(pw.schema_path, "requestBody.password");
        assert!(pw.constraints.contains(&"minLength: 8".to_string()));

        // allOf merge: properties and required of both parts.
        let customers = SpecRef {
            method: Method::Post,
            path: "/customers".into(),
        };
        let id = field_info(&spec, &customers, BodySide::Response(201), &[k("id")]).unwrap();
        assert!(id.required);
        assert!(id.constraints.contains(&"format: uuid".to_string()));
        let phone = field_info(&spec, &customers, BodySide::Response(201), &[k("phone")]).unwrap();
        assert_eq!(phone.schema_path, "CustomerCreate.phone");
        assert!(phone.constraints.iter().any(|c| c.starts_with("pattern: ^")));

        // Nullable inline property in a list response.
        let list = SpecRef {
            method: Method::Get,
            path: "/products".into(),
        };
        let next = field_info(&spec, &list, BodySide::Response(200), &[k("nextCursor")]).unwrap();
        assert_eq!(next.schema_path, "responses.200.nextCursor");
        assert!(next.constraints.contains(&"nullable".to_string()));
        let name = field_info(
            &spec,
            &list,
            BodySide::Response(200),
            &[k("data"), PathSegment::Index(3), k("name")],
        )
        .unwrap();
        assert_eq!(name.schema_path, "Product.name");
        assert!(name.required);
    }

    #[test]
    fn param_info_from_shop_api() {
        let spec = shop();
        let list = SpecRef {
            method: Method::Get,
            path: "/orders".into(),
        };
        let limit = param_info(&spec, &list, "limit", "query").unwrap();
        assert_eq!(limit.type_label, "integer");
        assert!(!limit.required);
        assert_eq!(
            limit.constraints,
            vec!["min: 1".to_string(), "max: 100".into(), "default: 20".into()]
        );
        assert_eq!(limit.location.as_deref(), Some("query"));
        assert_eq!(
            limit.description.as_deref(),
            Some("Максимальное количество заказов на странице")
        );
        let status = param_info(&spec, &list, "status", "query").unwrap();
        assert!(status.constraints[0].starts_with("enum: pending"));
        let cursor = param_info(&spec, &list, "cursor", "query").unwrap();
        assert_eq!(cursor.schema_path, "parameters.Cursor");
        assert!(param_info(&spec, &list, "limit", "header").is_none());

        let one = SpecRef {
            method: Method::Get,
            path: "/orders/{id}".into(),
        };
        let id = param_info(&spec, &one, "id", "path").unwrap();
        assert!(id.required);
    }

    #[test]
    fn validates_request_body_against_schema() {
        let spec = shop();
        let body: Value =
            serde_json::from_str(r#"{"customerId": "c", "items": [{"productId": "p", "quantity": "2"}]}"#)
                .unwrap();
        let m = validate_request_body(&spec, &post_orders(), &body).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].path, "$.items[0].quantity");
    }
}
