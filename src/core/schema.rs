//! Pragmatic JSON-schema subset over OpenAPI 3.0/3.1 schemas (see PLAN.md, core::schema).
//!
//! * [`validate`] — checks a JSON value against a schema and returns [`Mismatch`]es
//!   with JSON paths (`$.items[0].quantity`) and line numbers in the
//!   `serde_json::to_string_pretty` rendering of the body.
//! * [`example_from_schema`] — synthesizes an example value ("Generate from schema").
//! * [`line_of_path`] — maps a JSON path to its 0-based line in pretty-printed JSON.
//!
//! Supported keywords: `$ref` (local `#/...` pointers, nested, cycle-safe), `type`
//! (string or 3.1 array, `nullable`), `enum`, `const`, `required`, `properties`,
//! `additionalProperties` (false or schema), `items`, `minItems`/`maxItems`,
//! `uniqueItems`, `minimum`/`maximum`, `exclusiveMinimum`/`exclusiveMaximum`
//! (3.0 boolean and 3.1 numeric forms), `multipleOf`, `minLength`/`maxLength`,
//! `minProperties`/`maxProperties`, `pattern` (built-in mini regex engine, see
//! [`regex_is_match`]), `format` (`date-time`, `date`, `time`, `uuid`, `email`,
//! `uri`/`url`, `ipv4`), `allOf`, `anyOf`, `oneOf` (best effort, no discriminator).
//!
//! Not supported (silently ignored): remote/relative-file `$ref`s, `not`,
//! `patternProperties`, `dependencies`/`dependentRequired`, `if/then/else`,
//! `prefixItems`, `contains`, `discriminator`, `readOnly`/`writeOnly` semantics.

use serde_json::{Map, Value};

use crate::core::types::Mismatch;

/// Hard limit on schema nesting / `$ref` hops while validating or generating.
const MAX_DEPTH: usize = 64;
/// How deep example synthesis goes before cutting branches off.
const MAX_EXAMPLE_DEPTH: usize = 12;

// ---------------------------------------------------------------------------
// $ref resolution

/// Resolve a local JSON pointer reference like `#/components/schemas/Order`
/// against the whole OpenAPI document.
pub fn resolve_ref<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let pointer = reference.strip_prefix('#')?;
    if pointer.is_empty() {
        return Some(root);
    }
    // JSON pointer escapes, plus percent-encoded characters that sometimes appear.
    let decoded = percent_decode(pointer);
    root.pointer(&decoded)
}

fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Follow a chain of `$ref`s (guarded against cycles). Returns the final schema.
pub fn deref<'a>(root: &'a Value, mut schema: &'a Value) -> &'a Value {
    for _ in 0..MAX_DEPTH {
        match schema
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|r| resolve_ref(root, r))
        {
            Some(next) => schema = next,
            None => break,
        }
    }
    schema
}

/// Last segment of a `$ref`: `#/components/schemas/Order` → `Order`.
pub fn ref_name(reference: &str) -> &str {
    reference.rsplit('/').next().unwrap_or(reference)
}

// ---------------------------------------------------------------------------
// Validation

/// Validate `body` against `schema`. `root` is the whole OpenAPI document,
/// used to resolve `$ref`s. Returns an empty vec when the body matches.
pub fn validate(root: &Value, schema: &Value, body: &Value) -> Vec<Mismatch> {
    let mut out = Vec::new();
    validate_node(root, schema, body, "$", 0, &mut out);
    if !out.is_empty() {
        let pretty = serde_json::to_string_pretty(body).unwrap_or_default();
        for m in &mut out {
            m.line = line_of_existing_prefix(&pretty, body, &m.path);
        }
    }
    out
}

fn mismatch(
    path: &str,
    title: &str,
    expected: impl Into<String>,
    actual: impl Into<String>,
) -> Mismatch {
    Mismatch {
        path: path.to_string(),
        title: title.to_string(),
        expected: expected.into(),
        actual: actual.into(),
        line: None,
    }
}

fn validate_node(
    root: &Value,
    schema: &Value,
    node: &Value,
    path: &str,
    depth: usize,
    out: &mut Vec<Mismatch>,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let Some(obj) = schema.as_object() else {
        // 3.1 boolean schemas.
        if schema == &Value::Bool(false) {
            out.push(mismatch(
                path,
                "Значение запрещено схемой",
                "ничего",
                describe(node),
            ));
        }
        return;
    };

    if let Some(r) = obj.get("$ref").and_then(Value::as_str) {
        if let Some(target) = resolve_ref(root, r) {
            validate_node(root, target, node, path, depth + 1, out);
        }
        // OpenAPI 3.0 ignores siblings of $ref; 3.1 allows them. Only keep going
        // when there is something besides metadata.
        if !obj.keys().any(|k| is_assertion_keyword(k)) {
            return;
        }
    }

    // --- null handling --------------------------------------------------
    if node.is_null() && allows_null(obj) {
        return;
    }

    // --- type -------------------------------------------------------------
    if let Some(types) = declared_types(obj) {
        if !types.is_empty() && !types.iter().any(|t| type_matches(t, node)) {
            let mut expected = types.join(" | ");
            if let Some(f) = obj.get("format").and_then(Value::as_str) {
                expected = format!("{expected} ({f})");
            }
            out.push(mismatch(path, "Неверный тип", expected, describe(node)));
            return;
        }
    }

    // --- enum / const -----------------------------------------------------
    if let Some(values) = obj.get("enum").and_then(Value::as_array) {
        if !values.iter().any(|v| json_eq(v, node)) {
            let expected = values.iter().map(short).collect::<Vec<_>>().join(" | ");
            out.push(mismatch(path, "Значение не из enum", expected, short(node)));
        }
    }
    if let Some(c) = obj.get("const") {
        if !json_eq(c, node) {
            out.push(mismatch(
                path,
                "Значение не равно const",
                short(c),
                short(node),
            ));
        }
    }

    // --- composition ----------------------------------------------------
    if let Some(all) = obj.get("allOf").and_then(Value::as_array) {
        for sub in all {
            validate_node(root, sub, node, path, depth + 1, out);
        }
    }
    if let Some(any) = obj.get("anyOf").and_then(Value::as_array) {
        let branches: Vec<Vec<Mismatch>> = any
            .iter()
            .map(|s| branch(root, s, node, path, depth))
            .collect();
        if !branches.is_empty() && !branches.iter().any(Vec::is_empty) {
            out.extend(best_branch(branches));
        }
    }
    if let Some(one) = obj.get("oneOf").and_then(Value::as_array) {
        let branches: Vec<Vec<Mismatch>> = one
            .iter()
            .map(|s| branch(root, s, node, path, depth))
            .collect();
        let passing = branches.iter().filter(|b| b.is_empty()).count();
        if !branches.is_empty() && passing == 0 {
            out.extend(best_branch(branches));
        } else if passing > 1 {
            out.push(mismatch(
                path,
                "Подходит под несколько схем oneOf",
                "ровно одна схема",
                format!("{passing} схем(ы)"),
            ));
        }
    }

    match node {
        Value::Number(_) => validate_number(obj, node, path, out),
        Value::String(s) => validate_string(obj, s, path, out),
        Value::Array(items) => validate_array(root, obj, items, path, depth, out),
        Value::Object(map) => validate_object(root, obj, map, path, depth, out),
        _ => {}
    }
}

fn is_assertion_keyword(k: &str) -> bool {
    !matches!(
        k,
        "$ref"
            | "description"
            | "summary"
            | "title"
            | "example"
            | "examples"
            | "default"
            | "deprecated"
            | "readOnly"
            | "writeOnly"
            | "xml"
            | "externalDocs"
            | "$comment"
    ) && !k.starts_with("x-")
}

fn branch(root: &Value, schema: &Value, node: &Value, path: &str, depth: usize) -> Vec<Mismatch> {
    let mut v = Vec::new();
    validate_node(root, schema, node, path, depth + 1, &mut v);
    v
}

fn best_branch(branches: Vec<Vec<Mismatch>>) -> Vec<Mismatch> {
    branches
        .into_iter()
        .min_by_key(Vec::len)
        .unwrap_or_default()
}

fn allows_null(obj: &Map<String, Value>) -> bool {
    if obj.get("nullable").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    if let Some(Value::Array(types)) = obj.get("type") {
        if types.iter().any(|t| t.as_str() == Some("null")) {
            return true;
        }
    }
    if let Some(Value::Array(values)) = obj.get("enum") {
        if values.iter().any(Value::is_null) {
            return true;
        }
    }
    false
}

fn declared_types(obj: &Map<String, Value>) -> Option<Vec<String>> {
    match obj.get("type")? {
        Value::String(s) => Some(vec![s.clone()]),
        Value::Array(a) => Some(
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
        ),
        _ => None,
    }
}

fn type_matches(t: &str, node: &Value) -> bool {
    match t {
        "string" => node.is_string(),
        "number" => node.is_number(),
        "integer" => is_integer(node),
        "boolean" => node.is_boolean(),
        "array" => node.is_array(),
        "object" => node.is_object(),
        "null" => node.is_null(),
        _ => true,
    }
}

fn is_integer(node: &Value) -> bool {
    match node {
        Value::Number(n) => {
            n.is_i64()
                || n.is_u64()
                || n.as_f64()
                    .is_some_and(|f| f.fract() == 0.0 && f.is_finite())
        }
        _ => false,
    }
}

fn json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

/// JSON type name of a value.
pub fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) if is_integer(v) => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Compact JSON rendering, truncated.
fn short(v: &Value) -> String {
    let s = match v {
        Value::Array(a) => format!("[…{} элем.]", a.len()),
        Value::Object(o) => format!("{{…{} полей}}", o.len()),
        _ => v.to_string(),
    };
    if s.chars().count() > 60 {
        format!("{}…", s.chars().take(59).collect::<String>())
    } else {
        s
    }
}

/// `"2" (string)`.
fn describe(v: &Value) -> String {
    match v {
        Value::Array(_) | Value::Object(_) | Value::Null => type_name(v).to_string(),
        _ => format!("{} ({})", short(v), type_name(v)),
    }
}

fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
}

fn fmt_num(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{}", f as i64)
    } else {
        format!("{f}")
    }
}

fn validate_number(obj: &Map<String, Value>, node: &Value, path: &str, out: &mut Vec<Mismatch>) {
    let Some(x) = node.as_f64() else { return };
    let actual = short(node);
    if let Some(min) = num(obj.get("minimum")) {
        let exclusive = obj.get("exclusiveMinimum").and_then(Value::as_bool) == Some(true);
        if x < min || (exclusive && x == min) {
            let op = if exclusive { ">" } else { "≥" };
            out.push(mismatch(
                path,
                "Значение меньше минимума",
                format!("{op} {}", fmt_num(min)),
                actual.clone(),
            ));
        }
    }
    if let Some(min) = num(obj.get("exclusiveMinimum")) {
        if x <= min {
            out.push(mismatch(
                path,
                "Значение меньше минимума",
                format!("> {}", fmt_num(min)),
                actual.clone(),
            ));
        }
    }
    if let Some(max) = num(obj.get("maximum")) {
        let exclusive = obj.get("exclusiveMaximum").and_then(Value::as_bool) == Some(true);
        if x > max || (exclusive && x == max) {
            let op = if exclusive { "<" } else { "≤" };
            out.push(mismatch(
                path,
                "Значение больше максимума",
                format!("{op} {}", fmt_num(max)),
                actual.clone(),
            ));
        }
    }
    if let Some(max) = num(obj.get("exclusiveMaximum")) {
        if x >= max {
            out.push(mismatch(
                path,
                "Значение больше максимума",
                format!("< {}", fmt_num(max)),
                actual.clone(),
            ));
        }
    }
    if let Some(m) = num(obj.get("multipleOf")) {
        if m > 0.0 {
            let q = x / m;
            if (q - q.round()).abs() > 1e-9 {
                out.push(mismatch(
                    path,
                    "Значение не кратно multipleOf",
                    format!("кратно {}", fmt_num(m)),
                    actual,
                ));
            }
        }
    }
}

fn validate_string(obj: &Map<String, Value>, s: &str, path: &str, out: &mut Vec<Mismatch>) {
    let len = s.chars().count();
    if let Some(min) = obj.get("minLength").and_then(Value::as_u64) {
        if (len as u64) < min {
            out.push(mismatch(
                path,
                "Строка слишком короткая",
                format!("длина ≥ {min}"),
                format!("длина {len}"),
            ));
        }
    }
    if let Some(max) = obj.get("maxLength").and_then(Value::as_u64) {
        if (len as u64) > max {
            out.push(mismatch(
                path,
                "Строка слишком длинная",
                format!("длина ≤ {max}"),
                format!("длина {len}"),
            ));
        }
    }
    if let Some(p) = obj.get("pattern").and_then(Value::as_str) {
        if regex_is_match(p, s) == Some(false) {
            out.push(mismatch(
                path,
                "Строка не соответствует шаблону",
                format!("pattern {p}"),
                short(&Value::String(s.into())),
            ));
        }
    }
    if let Some(f) = obj.get("format").and_then(Value::as_str) {
        if !format_ok(f, s) {
            out.push(mismatch(
                path,
                "Неверный формат",
                format!("string ({f})"),
                short(&Value::String(s.into())),
            ));
        }
    }
}

fn validate_array(
    root: &Value,
    obj: &Map<String, Value>,
    items: &[Value],
    path: &str,
    depth: usize,
    out: &mut Vec<Mismatch>,
) {
    if let Some(min) = obj.get("minItems").and_then(Value::as_u64) {
        if (items.len() as u64) < min {
            out.push(mismatch(
                path,
                "Слишком мало элементов",
                format!("≥ {min}"),
                items.len().to_string(),
            ));
        }
    }
    if let Some(max) = obj.get("maxItems").and_then(Value::as_u64) {
        if (items.len() as u64) > max {
            out.push(mismatch(
                path,
                "Слишком много элементов",
                format!("≤ {max}"),
                items.len().to_string(),
            ));
        }
    }
    if obj.get("uniqueItems").and_then(Value::as_bool) == Some(true) {
        'outer: for (i, a) in items.iter().enumerate() {
            for b in &items[i + 1..] {
                if json_eq(a, b) {
                    out.push(mismatch(
                        path,
                        "Элементы не уникальны",
                        "уникальные элементы",
                        short(a),
                    ));
                    break 'outer;
                }
            }
        }
    }
    if let Some(item_schema) = obj.get("items") {
        for (i, item) in items.iter().enumerate() {
            validate_node(
                root,
                item_schema,
                item,
                &format!("{path}[{i}]"),
                depth + 1,
                out,
            );
        }
    }
}

fn validate_object(
    root: &Value,
    obj: &Map<String, Value>,
    map: &Map<String, Value>,
    path: &str,
    depth: usize,
    out: &mut Vec<Mismatch>,
) {
    let props = obj.get("properties").and_then(Value::as_object);
    if let Some(required) = obj.get("required").and_then(Value::as_array) {
        for key in required.iter().filter_map(Value::as_str) {
            if !map.contains_key(key) {
                let expected = props
                    .and_then(|p| p.get(key))
                    .map(|s| schema_label(root, s))
                    .unwrap_or_else(|| "обязательное поле".into());
                out.push(mismatch(
                    &child_path(path, key),
                    "Отсутствует обязательное поле",
                    expected,
                    "отсутствует",
                ));
            }
        }
    }
    if let Some(min) = obj.get("minProperties").and_then(Value::as_u64) {
        if (map.len() as u64) < min {
            out.push(mismatch(
                path,
                "Слишком мало полей",
                format!("≥ {min}"),
                map.len().to_string(),
            ));
        }
    }
    if let Some(max) = obj.get("maxProperties").and_then(Value::as_u64) {
        if (map.len() as u64) > max {
            out.push(mismatch(
                path,
                "Слишком много полей",
                format!("≤ {max}"),
                map.len().to_string(),
            ));
        }
    }
    let additional = obj.get("additionalProperties");
    for (key, value) in map {
        let p = child_path(path, key);
        if let Some(prop_schema) = props.and_then(|ps| ps.get(key)) {
            validate_node(root, prop_schema, value, &p, depth + 1, out);
        } else {
            match additional {
                Some(Value::Bool(false)) => {
                    out.push(mismatch(
                        &p,
                        "Лишнее поле",
                        "поле не описано в схеме",
                        short(value),
                    ));
                }
                Some(s @ Value::Object(_)) => validate_node(root, s, value, &p, depth + 1, out),
                _ => {}
            }
        }
    }
}

/// Human label for a schema: `$ref Order`, `integer`, `array of $ref Item`, `string (uuid)`.
pub fn schema_label(_root: &Value, schema: &Value) -> String {
    schema_label_depth(schema, 0)
}

fn schema_label_depth(schema: &Value, depth: usize) -> String {
    if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
        return format!("$ref {}", ref_name(r));
    }
    let Some(obj) = schema.as_object() else {
        return "any".into();
    };
    let ty = match declared_types(obj) {
        Some(t) if !t.is_empty() => t.join(" | "),
        _ if obj.contains_key("properties") => "object".into(),
        _ if obj.contains_key("items") => "array".into(),
        _ if obj.contains_key("allOf") => "allOf".into(),
        _ if obj.contains_key("oneOf") => "oneOf".into(),
        _ if obj.contains_key("anyOf") => "anyOf".into(),
        _ => "any".into(),
    };
    if ty == "array" && depth < 4 {
        if let Some(items) = obj.get("items") {
            return format!("array of {}", schema_label_depth(items, depth + 1));
        }
    }
    match obj.get("format").and_then(Value::as_str) {
        Some(f) => format!("{ty} ({f})"),
        None => ty,
    }
}

// ---------------------------------------------------------------------------
// Formats

fn format_ok(format: &str, s: &str) -> bool {
    match format {
        "date-time" => is_date_time(s),
        "date" => is_date(s),
        "time" => is_time(s),
        "uuid" => is_uuid(s),
        "email" => is_email(s),
        "uri" | "url" => is_uri(s),
        "ipv4" => is_ipv4(s),
        _ => true,
    }
}

fn digits(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|b| b.is_ascii_digit())
}

fn is_date(s: &str) -> bool {
    let p: Vec<&str> = s.split('-').collect();
    if p.len() != 3 || !digits(p[0], 4) || !digits(p[1], 2) || !digits(p[2], 2) {
        return false;
    }
    let m: u32 = p[1].parse().unwrap_or(0);
    let d: u32 = p[2].parse().unwrap_or(0);
    (1..=12).contains(&m) && (1..=31).contains(&d)
}

/// `HH:MM:SS[.frac][Z|±HH:MM]` (offset optional for `time`).
fn is_time_with(s: &str, need_offset: bool) -> bool {
    if s.len() < 8 || !s.is_char_boundary(8) {
        return false;
    }
    let (hms, mut rest) = s.split_at(8);
    let p: Vec<&str> = hms.split(':').collect();
    if p.len() != 3 || !p.iter().all(|x| digits(x, 2)) {
        return false;
    }
    let (h, m, sec): (u32, u32, u32) = (
        p[0].parse().unwrap(),
        p[1].parse().unwrap(),
        p[2].parse().unwrap(),
    );
    if h > 23 || m > 59 || sec > 60 {
        return false;
    }
    if let Some(r) = rest.strip_prefix('.') {
        let n = r.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return false;
        }
        rest = &r[n..];
    }
    match rest {
        "" => !need_offset,
        "Z" | "z" => true,
        _ => {
            let Some(off) = rest.strip_prefix('+').or_else(|| rest.strip_prefix('-')) else {
                return false;
            };
            let q: Vec<&str> = off.split(':').collect();
            q.len() == 2 && digits(q[0], 2) && digits(q[1], 2)
        }
    }
}

fn is_time(s: &str) -> bool {
    is_time_with(s, false)
}

fn is_date_time(s: &str) -> bool {
    let Some(i) = s.find(['T', 't', ' ']) else {
        return false;
    };
    is_date(&s[..i]) && is_time_with(&s[i + 1..], true)
}

fn is_uuid(s: &str) -> bool {
    let groups: Vec<&str> = s.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn is_email(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.contains('@')
        && !s.chars().any(char::is_whitespace)
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
}

fn is_uri(s: &str) -> bool {
    let Some((scheme, rest)) = s.split_once(':') else {
        return false;
    };
    !scheme.is_empty()
        && scheme
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        && !rest.is_empty()
        && !s.chars().any(char::is_whitespace)
}

fn is_ipv4(s: &str) -> bool {
    let p: Vec<&str> = s.split('.').collect();
    p.len() == 4
        && p.iter()
            .all(|x| !x.is_empty() && x.len() <= 3 && x.parse::<u8>().is_ok())
}

// ---------------------------------------------------------------------------
// JSON paths and line numbers

#[derive(Debug, PartialEq)]
enum Seg {
    Key(String),
    Index(usize),
}

/// `$.a` for identifier-like keys, `$["a.b"]` otherwise.
fn child_path(parent: &str, key: &str) -> String {
    let ident = !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$' || c == '-')
        && !key.chars().next().is_some_and(|c| c.is_ascii_digit());
    if ident {
        format!("{parent}.{key}")
    } else {
        format!("{parent}[{}]", Value::String(key.to_string()))
    }
}

fn parse_path(path: &str) -> Option<Vec<Seg>> {
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
                segs.push(Seg::Key(chars[start..i].iter().collect()));
            }
            '[' => {
                if chars.get(i + 1) == Some(&'"') {
                    // Quoted key: read a JSON string literal.
                    let mut j = i + 2;
                    while j < chars.len() && chars[j] != '"' {
                        if chars[j] == '\\' {
                            j += 1;
                        }
                        j += 1;
                    }
                    let lit: String = chars[i + 1..=j.min(chars.len() - 1)].iter().collect();
                    let key: String = serde_json::from_str(&lit).ok()?;
                    if chars.get(j + 1) != Some(&']') {
                        return None;
                    }
                    segs.push(Seg::Key(key));
                    i = j + 2;
                } else {
                    let end = chars[i..].iter().position(|&c| c == ']')? + i;
                    let idx: usize = chars[i + 1..end].iter().collect::<String>().parse().ok()?;
                    segs.push(Seg::Index(idx));
                    i = end + 1;
                }
            }
            _ => return None,
        }
    }
    Some(segs)
}

/// Number of lines `serde_json::to_string_pretty` uses for `v`.
fn pretty_lines(v: &Value) -> usize {
    match v {
        Value::Array(a) if !a.is_empty() => 2 + a.iter().map(pretty_lines).sum::<usize>(),
        Value::Object(o) if !o.is_empty() => 2 + o.values().map(pretty_lines).sum::<usize>(),
        _ => 1,
    }
}

/// 0-based line of `path` (e.g. `$.items[0].quantity`) in `pretty`, which must be
/// `serde_json::to_string_pretty(body)`. Relies on serde_json's pretty layout:
/// one value per line, a key sits on the same line as the start of its value,
/// non-empty containers open on their own line and close on a separate one.
/// Returns `None` if the path does not exist in `body` (or `pretty` doesn't match).
pub fn line_of_path(pretty: &str, body: &Value, path: &str) -> Option<usize> {
    let segs = parse_path(path)?;
    let mut line = 0usize;
    let mut node = body;
    for seg in &segs {
        line += 1; // past the opening `{` / `[` line
        match (seg, node) {
            (Seg::Key(k), Value::Object(o)) => {
                let mut found = None;
                for (key, val) in o {
                    if key == k {
                        found = Some(val);
                        break;
                    }
                    line += pretty_lines(val);
                }
                node = found?;
            }
            (Seg::Index(i), Value::Array(a)) => {
                let val = a.get(*i)?;
                line += a[..*i].iter().map(pretty_lines).sum::<usize>();
                node = val;
            }
            _ => return None,
        }
    }
    // Sanity check against the actual text.
    if line >= pretty.lines().count().max(1) {
        return None;
    }
    Some(line)
}

/// Line of the path, or of its deepest existing ancestor (for missing fields).
fn line_of_existing_prefix(pretty: &str, body: &Value, path: &str) -> Option<usize> {
    let mut p = path.to_string();
    loop {
        if let Some(l) = line_of_path(pretty, body, &p) {
            return Some(l);
        }
        let cut = p.rfind(['.', '['])?;
        if cut == 0 {
            return None;
        }
        p.truncate(cut);
    }
}

// ---------------------------------------------------------------------------
// Example synthesis

/// Build an example value for `schema` (`root` = whole document for `$ref`s).
/// Uses `example` / `examples` / `default` / `const` / first `enum` value when
/// present, otherwise synthesizes: format-aware strings, numbers respecting
/// `minimum`, objects with all properties, arrays with one item (or `minItems`).
/// Recursive `$ref`s are cut off (property omitted, array left empty).
pub fn example_from_schema(root: &Value, schema: &Value) -> Value {
    let mut stack = Vec::new();
    example_inner(root, schema, 0, &mut stack).unwrap_or(Value::Null)
}

fn example_inner(
    root: &Value,
    schema: &Value,
    depth: usize,
    stack: &mut Vec<String>,
) -> Option<Value> {
    if depth > MAX_EXAMPLE_DEPTH {
        return None;
    }
    let obj = schema.as_object()?;
    if let Some(r) = obj.get("$ref").and_then(Value::as_str) {
        if stack.iter().any(|s| s == r) {
            return None; // recursion
        }
        let target = resolve_ref(root, r)?;
        stack.push(r.to_string());
        let v = example_inner(root, target, depth + 1, stack);
        stack.pop();
        return v;
    }
    if let Some(e) = obj.get("example") {
        return Some(e.clone());
    }
    if let Some(e) = obj
        .get("examples")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
    {
        return Some(e.clone());
    }
    if let Some(e) = obj.get("default") {
        return Some(e.clone());
    }
    if let Some(e) = obj.get("const") {
        return Some(e.clone());
    }
    if let Some(e) = obj
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().find(|v| !v.is_null()))
    {
        return Some(e.clone());
    }
    if let Some(all) = obj.get("allOf").and_then(Value::as_array) {
        let mut merged = Map::new();
        let mut last = None;
        for sub in all {
            match example_inner(root, sub, depth + 1, stack) {
                Some(Value::Object(o)) => merged.extend(o),
                Some(v) => last = Some(v),
                None => {}
            }
        }
        // Properties declared next to allOf.
        if obj.contains_key("properties") {
            if let Some(Value::Object(o)) = object_example(root, obj, depth, stack) {
                merged.extend(o);
            }
        }
        return if merged.is_empty() {
            last.or(Some(Value::Object(merged)))
        } else {
            Some(Value::Object(merged))
        };
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(alts) = obj.get(key).and_then(Value::as_array) {
            if let Some(v) = alts
                .iter()
                .find_map(|s| example_inner(root, s, depth + 1, stack))
            {
                return Some(v);
            }
        }
    }

    let ty = declared_types(obj)
        .and_then(|t| t.into_iter().find(|t| t != "null"))
        .or_else(|| {
            if obj.contains_key("properties") || obj.contains_key("additionalProperties") {
                Some("object".into())
            } else if obj.contains_key("items") {
                Some("array".into())
            } else {
                None
            }
        });
    match ty.as_deref() {
        Some("string") => Some(Value::String(string_example(obj))),
        Some("integer") => Some(Value::from(integer_example(obj))),
        Some("number") => Some(number_example(obj)),
        Some("boolean") => Some(Value::Bool(true)),
        Some("array") => {
            let count = obj
                .get("minItems")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .clamp(1, 3) as usize;
            let items = match obj.get("items") {
                Some(item) => match example_inner(root, item, depth + 1, stack) {
                    Some(v) => vec![v; count],
                    None => Vec::new(),
                },
                None => Vec::new(),
            };
            Some(Value::Array(items))
        }
        Some("object") => object_example(root, obj, depth, stack),
        Some("null") => Some(Value::Null),
        _ => Some(Value::Null),
    }
}

fn object_example(
    root: &Value,
    obj: &Map<String, Value>,
    depth: usize,
    stack: &mut Vec<String>,
) -> Option<Value> {
    let mut out = Map::new();
    if let Some(props) = obj.get("properties").and_then(Value::as_object) {
        for (k, s) in props {
            if let Some(v) = example_inner(root, s, depth + 1, stack) {
                out.insert(k.clone(), v);
            }
        }
    } else if let Some(add @ Value::Object(_)) = obj.get("additionalProperties") {
        if let Some(v) = example_inner(root, add, depth + 1, stack) {
            out.insert("key".into(), v);
        }
    }
    Some(Value::Object(out))
}

fn string_example(obj: &Map<String, Value>) -> String {
    let base = match obj.get("format").and_then(Value::as_str) {
        Some("date-time") => "2024-01-01T12:00:00Z",
        Some("date") => "2024-01-01",
        Some("time") => "12:00:00",
        Some("uuid") => "3fa85f64-5717-4562-b3fc-2c963f66afa6",
        Some("email") => "user@example.com",
        Some("uri") | Some("url") => "https://example.com",
        Some("hostname") => "example.com",
        Some("ipv4") => "192.168.0.1",
        Some("ipv6") => "::1",
        Some("byte") => "c3RyaW5n",
        Some("password") => "password",
        _ => "string",
    };
    let mut s = base.to_string();
    if let Some(min) = obj.get("minLength").and_then(Value::as_u64) {
        while (s.chars().count() as u64) < min {
            s.push('x');
        }
    }
    if let Some(max) = obj.get("maxLength").and_then(Value::as_u64) {
        if (s.chars().count() as u64) > max {
            s = s.chars().take(max as usize).collect();
        }
    }
    s
}

fn integer_example(obj: &Map<String, Value>) -> i64 {
    let mut v = 0i64;
    if let Some(min) = num(obj.get("minimum")) {
        v = min.ceil() as i64;
        if obj.get("exclusiveMinimum").and_then(Value::as_bool) == Some(true) && (v as f64) <= min {
            v += 1;
        }
    }
    if let Some(min) = num(obj.get("exclusiveMinimum")) {
        if (v as f64) <= min {
            v = min.floor() as i64 + 1;
        }
    }
    if let Some(max) = num(obj.get("maximum")) {
        if v as f64 > max {
            v = max.floor() as i64;
        }
    }
    v
}

fn number_example(obj: &Map<String, Value>) -> Value {
    let min = num(obj.get("minimum")).or_else(|| num(obj.get("exclusiveMinimum")).map(|m| m + 1.0));
    let max = num(obj.get("maximum"));
    let v = match (min, max) {
        (Some(m), _) => m,
        (None, Some(m)) if m < 0.0 => m,
        _ => 0.0,
    };
    if v.fract() == 0.0 && v.abs() < 1e15 {
        Value::from(v as i64)
    } else {
        Value::from(v)
    }
}

// ---------------------------------------------------------------------------
// Mini regex engine for `pattern`
//
// Supports: literals, `.`, `^`, `$`, escapes `\d \w \s \D \W \S` and escaped
// metacharacters, `\n \t \r`, character classes `[a-z_]` / `[^...]` (with escapes
// inside), groups `( )` / `(?: )`, alternation `|`, quantifiers `* + ? {n} {n,}
// {n,m}` (lazy `?` suffix accepted). Unanchored search, as JSON Schema requires.
// Anything else (backreferences, lookaround, `\b`, unicode classes) makes the
// pattern "unsupported" and it is skipped.

#[derive(Debug)]
enum RxNode {
    Char(char),
    Any,
    Class(Vec<ClassItem>, bool),
    Start,
    End,
    Group(Vec<Vec<RxItem>>),
}

#[derive(Debug)]
enum ClassItem {
    Range(char, char),
    Digit(bool),
    Word(bool),
    Space(bool),
}

#[derive(Debug)]
struct RxItem {
    node: RxNode,
    min: usize,
    max: Option<usize>,
}

struct RxParser<'a> {
    c: &'a [char],
    i: usize,
}

impl RxParser<'_> {
    fn alt(&mut self, depth: usize) -> Option<Vec<Vec<RxItem>>> {
        if depth > 32 {
            return None;
        }
        let mut alts = vec![self.seq(depth)?];
        while self.c.get(self.i) == Some(&'|') {
            self.i += 1;
            alts.push(self.seq(depth)?);
        }
        Some(alts)
    }

    fn seq(&mut self, depth: usize) -> Option<Vec<RxItem>> {
        let mut items = Vec::new();
        while let Some(&ch) = self.c.get(self.i) {
            if ch == '|' || ch == ')' {
                break;
            }
            self.i += 1;
            let node = match ch {
                '.' => RxNode::Any,
                '^' => RxNode::Start,
                '$' => RxNode::End,
                '(' => {
                    if self.c.get(self.i) == Some(&'?') {
                        if self.c.get(self.i + 1) == Some(&':') {
                            self.i += 2;
                        } else {
                            return None;
                        }
                    }
                    let g = self.alt(depth + 1)?;
                    if self.c.get(self.i) != Some(&')') {
                        return None;
                    }
                    self.i += 1;
                    RxNode::Group(g)
                }
                '[' => self.class()?,
                '\\' => match self.escape()? {
                    Esc::Char(c) => RxNode::Char(c),
                    Esc::Class(ci) => RxNode::Class(vec![ci], false),
                },
                '*' | '+' | '?' | '{' => return None,
                c => RxNode::Char(c),
            };
            let (min, max) = self.quantifier()?;
            items.push(RxItem { node, min, max });
        }
        Some(items)
    }

    fn quantifier(&mut self) -> Option<(usize, Option<usize>)> {
        let q = match self.c.get(self.i) {
            Some('*') => {
                self.i += 1;
                (0, None)
            }
            Some('+') => {
                self.i += 1;
                (1, None)
            }
            Some('?') => {
                self.i += 1;
                (0, Some(1))
            }
            Some('{') => {
                let end = self.c[self.i..].iter().position(|&c| c == '}')? + self.i;
                let body: String = self.c[self.i + 1..end].iter().collect();
                let (a, b) = match body.split_once(',') {
                    Some((a, "")) => (a.trim().parse().ok()?, None),
                    Some((a, b)) => (a.trim().parse().ok()?, Some(b.trim().parse().ok()?)),
                    None => {
                        let n: usize = body.trim().parse().ok()?;
                        (n, Some(n))
                    }
                };
                self.i = end + 1;
                (a, b)
            }
            _ => return Some((1, Some(1))),
        };
        if self.c.get(self.i) == Some(&'?') {
            self.i += 1; // lazy: irrelevant for match/no-match
        }
        Some(q)
    }

    fn escape(&mut self) -> Option<Esc> {
        let ch = *self.c.get(self.i)?;
        self.i += 1;
        Some(match ch {
            'd' => Esc::Class(ClassItem::Digit(true)),
            'D' => Esc::Class(ClassItem::Digit(false)),
            'w' => Esc::Class(ClassItem::Word(true)),
            'W' => Esc::Class(ClassItem::Word(false)),
            's' => Esc::Class(ClassItem::Space(true)),
            'S' => Esc::Class(ClassItem::Space(false)),
            'n' => Esc::Char('\n'),
            't' => Esc::Char('\t'),
            'r' => Esc::Char('\r'),
            c if c.is_ascii_alphanumeric() => return None,
            c => Esc::Char(c),
        })
    }

    fn class(&mut self) -> Option<RxNode> {
        let mut negated = false;
        if self.c.get(self.i) == Some(&'^') {
            negated = true;
            self.i += 1;
        }
        let mut items = Vec::new();
        let mut first = true;
        loop {
            let ch = *self.c.get(self.i)?;
            self.i += 1;
            if ch == ']' && !first {
                break;
            }
            first = false;
            let lo = if ch == '\\' {
                match self.escape()? {
                    Esc::Char(c) => c,
                    Esc::Class(ci) => {
                        items.push(ci);
                        continue;
                    }
                }
            } else {
                ch
            };
            if self.c.get(self.i) == Some(&'-') && self.c.get(self.i + 1).is_some_and(|&c| c != ']')
            {
                self.i += 1;
                let mut hi = *self.c.get(self.i)?;
                self.i += 1;
                if hi == '\\' {
                    match self.escape()? {
                        Esc::Char(c) => hi = c,
                        Esc::Class(_) => return None,
                    }
                }
                if hi < lo {
                    return None;
                }
                items.push(ClassItem::Range(lo, hi));
            } else {
                items.push(ClassItem::Range(lo, lo));
            }
        }
        Some(RxNode::Class(items, negated))
    }
}

enum Esc {
    Char(char),
    Class(ClassItem),
}

fn class_item_matches(ci: &ClassItem, c: char) -> bool {
    match *ci {
        ClassItem::Range(a, b) => a <= c && c <= b,
        ClassItem::Digit(pos) => c.is_ascii_digit() == pos,
        ClassItem::Word(pos) => (c.is_ascii_alphanumeric() || c == '_') == pos,
        ClassItem::Space(pos) => c.is_whitespace() == pos,
    }
}

struct Matcher<'a> {
    s: &'a [char],
    steps: usize,
}

const RX_STEP_LIMIT: usize = 200_000;

impl Matcher<'_> {
    fn alt(
        &mut self,
        alts: &[Vec<RxItem>],
        pos: usize,
        k: &mut dyn FnMut(&mut Self, usize) -> bool,
    ) -> bool {
        for seq in alts {
            if self.seq(seq, pos, k) {
                return true;
            }
        }
        false
    }

    fn seq(
        &mut self,
        seq: &[RxItem],
        pos: usize,
        k: &mut dyn FnMut(&mut Self, usize) -> bool,
    ) -> bool {
        match seq.split_first() {
            None => k(self, pos),
            Some((first, rest)) => {
                self.item(first, 0, pos, &mut |m: &mut Self, p| m.seq(rest, p, k))
            }
        }
    }

    fn item(
        &mut self,
        it: &RxItem,
        count: usize,
        pos: usize,
        k: &mut dyn FnMut(&mut Self, usize) -> bool,
    ) -> bool {
        self.steps += 1;
        if self.steps > RX_STEP_LIMIT {
            return false;
        }
        if it.max.is_none_or(|m| count < m) {
            let ok = self.node(&it.node, pos, &mut |m: &mut Self, p| {
                if p == pos && count >= it.min {
                    false // empty iteration: avoid infinite loops
                } else {
                    m.item(it, count + 1, p, k)
                }
            });
            if ok {
                return true;
            }
        }
        count >= it.min && k(self, pos)
    }

    fn node(
        &mut self,
        node: &RxNode,
        pos: usize,
        k: &mut dyn FnMut(&mut Self, usize) -> bool,
    ) -> bool {
        match node {
            RxNode::Start => pos == 0 && k(self, pos),
            RxNode::End => pos == self.s.len() && k(self, pos),
            RxNode::Group(alts) => self.alt(alts, pos, k),
            _ => {
                let Some(&c) = self.s.get(pos) else {
                    return false;
                };
                let ok = match node {
                    RxNode::Char(x) => *x == c,
                    RxNode::Any => c != '\n',
                    RxNode::Class(items, neg) => {
                        items.iter().any(|ci| class_item_matches(ci, c)) != *neg
                    }
                    _ => false,
                };
                ok && k(self, pos + 1)
            }
        }
    }
}

/// Unanchored regex search with the built-in mini engine. `None` when the
/// pattern uses unsupported syntax or is too expensive to evaluate (callers
/// then skip the check).
pub fn regex_is_match(pattern: &str, text: &str) -> Option<bool> {
    let pc: Vec<char> = pattern.chars().collect();
    let mut parser = RxParser { c: &pc, i: 0 };
    let alts = parser.alt(0)?;
    if parser.i != pc.len() {
        return None;
    }
    let s: Vec<char> = text.chars().collect();
    let mut m = Matcher { s: &s, steps: 0 };
    for start in 0..=s.len() {
        if m.alt(&alts, start, &mut |_, _| true) {
            return Some(true);
        }
        if m.steps > RX_STEP_LIMIT {
            return None;
        }
    }
    Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn regex_basics() {
        assert_eq!(
            regex_is_match(r"^\+[0-9]{10,15}$", "+79001234567"),
            Some(true)
        );
        assert_eq!(
            regex_is_match(r"^\+[0-9]{10,15}$", "79001234567"),
            Some(false)
        );
        assert_eq!(regex_is_match(r"^(ab|cd)+x?$", "abcdab"), Some(true));
        assert_eq!(regex_is_match(r"^(?:ab|cd)+$", "abce"), Some(false));
        assert_eq!(regex_is_match(r"\d-\w", "zz1-a"), Some(true));
        assert_eq!(regex_is_match(r"^[^a-c]*$", "xyz"), Some(true));
        assert_eq!(regex_is_match(r"^[^a-c]*$", "xbz"), Some(false));
        assert_eq!(regex_is_match(r"(?=x)", "x"), None);
        // Pathological backtracking stays bounded (either answer or "unsupported").
        assert_ne!(
            regex_is_match(r"^(a*)*a*a*a*b$", &"a".repeat(40)),
            Some(true)
        );
    }

    #[test]
    fn formats() {
        assert!(is_date_time("2026-10-15T12:30:00Z"));
        assert!(is_date_time("2026-10-15T12:30:00.123+03:00"));
        assert!(!is_date_time("2026-10-15"));
        assert!(!is_date_time("yesterday"));
        assert!(is_uuid("3fa85f64-5717-4562-b3fc-2c963f66afa6"));
        assert!(!is_uuid("3fa85f64"));
        assert!(is_email("a@b.co"));
        assert!(!is_email("a@b"));
    }

    #[test]
    fn lines_follow_pretty_layout() {
        let body = json!({"a": 1, "b": {"c": [1, {"d": 2}], "e": []}, "f": "x"});
        let pretty = serde_json::to_string_pretty(&body).unwrap();
        let lines: Vec<&str> = pretty.lines().collect();
        for (path, needle) in [
            ("$", "{"),
            ("$.a", "\"a\""),
            ("$.b", "\"b\""),
            ("$.b.c", "\"c\""),
            ("$.b.c[1].d", "\"d\""),
            ("$.b.e", "\"e\""),
            ("$.f", "\"f\""),
        ] {
            let l = line_of_path(&pretty, &body, path).unwrap();
            assert!(lines[l].contains(needle), "{path}: {}", lines[l]);
        }
        assert_eq!(line_of_path(&pretty, &body, "$.b.c[0]"), Some(4));
        assert_eq!(line_of_path(&pretty, &body, "$.zzz"), None);
    }

    #[test]
    fn quoted_keys_in_paths() {
        let body = json!({"a.b": {"x y": true}});
        let pretty = serde_json::to_string_pretty(&body).unwrap();
        assert_eq!(child_path("$", "a.b"), "$[\"a.b\"]");
        assert_eq!(line_of_path(&pretty, &body, "$[\"a.b\"][\"x y\"]"), Some(2));
    }

    #[test]
    fn composition_and_nullable() {
        let root = json!({});
        let s = json!({"anyOf": [{"type": "string"}, {"type": "integer"}]});
        assert!(validate(&root, &s, &json!(3)).is_empty());
        assert_eq!(validate(&root, &s, &json!(true)).len(), 1);
        let s = json!({"type": "string", "nullable": true});
        assert!(validate(&root, &s, &Value::Null).is_empty());
        let s = json!({"type": ["string", "null"]});
        assert!(validate(&root, &s, &Value::Null).is_empty());
        let s = json!({"type": "object", "additionalProperties": false, "properties": {"a": {}}});
        let m = validate(&root, &s, &json!({"a": 1, "b": 2}));
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].path, "$.b");
        assert_eq!(m[0].line, Some(2));
    }
}
