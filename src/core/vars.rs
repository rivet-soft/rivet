//! See PLAN.md (core::vars).
//!
//! `{{name}}` substitution from the active environment plus built-ins
//! (`{{$timestamp}}`, `{{$uuid}}`, `{{$randomInt}}`), and [`resolve`], which turns a
//! [`RequestSpec`] into a ready-to-send [`ResolvedRequest`].

use super::types::{Auth, BodyKind, Environment, RequestSpec, ResolvedRequest, SendError};

/// Iterates `{{ ... }}` placeholders: yields `(start, end, trimmed_name)` byte ranges
/// (the range covers the braces). Empty names are skipped.
fn placeholders(text: &str) -> Vec<(usize, usize, &str)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(off) = text[i..].find("{{") {
        let start = i + off;
        let inner_start = start + 2;
        let Some(close) = text[inner_start..].find("}}") else {
            break;
        };
        let inner = &text[inner_start..inner_start + close];
        // A nested `{{` means the first one was a stray brace: restart from the inner one.
        if let Some(nested) = inner.rfind("{{") {
            i = inner_start + nested;
            continue;
        }
        let end = inner_start + close + 2;
        let name = inner.trim();
        if !name.is_empty() {
            out.push((start, end, name));
        }
        i = end;
    }
    out
}

fn push_unique(list: &mut Vec<String>, name: &str) {
    if !list.iter().any(|n| n == name) {
        list.push(name.to_string());
    }
}

/// Environment variable names referenced as `{{name}}` (unique, in order of
/// appearance). Built-ins (`{{$...}}`) are not included.
pub fn referenced_vars(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (_, _, name) in placeholders(text) {
        if !name.starts_with('$') {
            push_unique(&mut out, name);
        }
    }
    out
}

fn builtin(name: &str) -> Option<String> {
    match name {
        "$timestamp" => Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
                .to_string(),
        ),
        "$uuid" | "$guid" => Some(uuid::Uuid::new_v4().to_string()),
        "$randomInt" => {
            let b = uuid::Uuid::new_v4().into_bytes();
            Some((u32::from_le_bytes([b[0], b[1], b[2], b[3]]) % 1001).to_string())
        }
        _ => None,
    }
}

/// Replaces `{{name}}` with enabled environment variables and built-ins.
/// Unknown names are left in place and returned as missing (unique, in order).
pub fn substitute(text: &str, env: Option<&Environment>) -> (String, Vec<String>) {
    let mut current = text.to_string();
    for _ in 0..8 {
        let (next, mut missing) = substitute_once(&current, env);
        if next == current {
            // A variable that expands to itself is a cycle, not a valid value.
            for (_, _, name) in placeholders(&next) {
                push_unique(&mut missing, name);
            }
            return (next, missing);
        }
        current = next;
    }
    let mut missing = Vec::new();
    for (_, _, name) in placeholders(&current) {
        push_unique(&mut missing, name);
    }
    (current, missing)
}

fn substitute_once(text: &str, env: Option<&Environment>) -> (String, Vec<String>) {
    let mut out = String::with_capacity(text.len());
    let mut missing = Vec::new();
    let mut last = 0;
    for (start, end, name) in placeholders(text) {
        let value = if name.starts_with('$') {
            builtin(name)
        } else {
            env.and_then(|e| e.var(name)).map(str::to_string)
        };
        out.push_str(&text[last..start]);
        match value {
            Some(v) => out.push_str(&v),
            None => {
                out.push_str(&text[start..end]);
                push_unique(&mut missing, name);
            }
        }
        last = end;
    }
    out.push_str(&text[last..]);
    (out, missing)
}

/// Percent-encodes everything except RFC 3986 unreserved characters.
pub(crate) fn encode_component(s: &str, space_as_plus: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            b' ' if space_as_plus => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Decodes `%XX` (and `+` as space when `plus_as_space`); invalid escapes stay literal.
pub(crate) fn decode_component(s: &str, plus_as_space: bool) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(v) = hex {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if b == b'+' && plus_as_space { b' ' } else { b });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Escapes characters that may not appear raw in a URL (spaces, controls, non-ASCII…),
/// leaving existing `%XX` escapes and URL syntax untouched.
fn normalize_url_chars(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b <= 0x20
            || b >= 0x7F
            || matches!(
                b,
                b'"' | b'<' | b'>' | b'\\' | b'^' | b'`' | b'{' | b'|' | b'}'
            )
        {
            out.push_str(&format!("%{b:02X}"));
        } else {
            out.push(b as char);
        }
    }
    out
}

/// Minimal standard base64 (with padding).
pub(crate) fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Validates/normalizes an already-substituted URL: adds `http://` when the scheme
/// is missing, drops the fragment, escapes illegal characters.
fn normalize_url(raw: &str) -> Result<String, SendError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(SendError::InvalidUrl(raw.to_string()));
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };
    let without_fragment = match with_scheme.find('#') {
        Some(i) => &with_scheme[..i],
        None => &with_scheme[..],
    };
    let (scheme, rest) = without_fragment
        .split_once("://")
        .ok_or_else(|| SendError::InvalidUrl(raw.to_string()))?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(SendError::InvalidUrl(raw.to_string()));
    }
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if host_port.starts_with('[') {
        host_port
            .split_once(']')
            .map_or("", |(h, _)| h.trim_start_matches('['))
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    if host.is_empty() || host.contains(char::is_whitespace) || host.contains("{{") {
        return Err(SendError::InvalidUrl(raw.to_string()));
    }
    if let Some((_, port)) = host_port
        .rsplit_once(':')
        .filter(|_| !host_port.ends_with(']'))
    {
        if !port.is_empty() && port.parse::<u16>().is_err() && !host_port.starts_with('[') {
            return Err(SendError::InvalidUrl(raw.to_string()));
        }
    }
    let mut path = rest[authority_end..].to_string();
    if path.is_empty() || path.starts_with('?') {
        path.insert(0, '/');
    }
    Ok(format!(
        "{}://{}{}",
        scheme.to_ascii_lowercase(),
        authority,
        normalize_url_chars(&path)
    ))
}

fn append_query(url: &mut String, pairs: &[(String, String)]) {
    if pairs.is_empty() {
        return;
    }
    if !url.contains('?') {
        url.push('?');
    } else if !url.ends_with('?') && !url.ends_with('&') {
        url.push('&');
    }
    let encoded: Vec<String> = pairs
        .iter()
        .map(|(k, v)| {
            format!(
                "{}={}",
                encode_component(k, false),
                encode_component(v, false)
            )
        })
        .collect();
    url.push_str(&encoded.join("&"));
}

fn has_header(headers: &[(String, String)], name: &str) -> bool {
    headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(name))
}

fn file_mime(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "json" => "application/json",
        "txt" | "log" | "csv" => "text/plain",
        "html" => "text/html",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

fn read_body_file(path: &str) -> Result<Vec<u8>, SendError> {
    std::fs::read(path).map_err(|e| SendError::File {
        path: path.into(),
        message: e.to_string(),
    })
}

fn disposition_value(value: &str) -> String {
    value.replace(['\r', '\n', '"', '\\'], "_")
}

/// Applies vars, params, headers, body and auth: produces an absolute URL with the
/// query string, the final header list and the encoded body. Missing variables are
/// collected in `missing_vars` (not an error); an empty or malformed URL is
/// `SendError::InvalidUrl`.
pub fn resolve(req: &RequestSpec, env: Option<&Environment>) -> Result<ResolvedRequest, SendError> {
    let mut missing = Vec::new();
    let mut sub = |text: &str| {
        let (s, m) = substitute(text, env);
        for name in m {
            push_unique(&mut missing, &name);
        }
        s
    };

    let raw_url = sub(&req.url);
    let mut url = normalize_url(&raw_url)?;

    let mut query: Vec<(String, String)> = req
        .params
        .iter()
        .filter(|p| p.enabled && !p.key.trim().is_empty())
        .map(|p| (sub(&p.key), sub(&p.value)))
        .collect();

    let mut headers: Vec<(String, String)> = req
        .headers
        .iter()
        .filter(|h| h.enabled && !h.key.trim().is_empty())
        .map(|h| (sub(h.key.trim()), sub(&h.value)))
        .collect();

    let body = match req.body.kind {
        BodyKind::None => None,
        BodyKind::Json | BodyKind::Text => {
            let text = sub(&req.body.text);
            if !has_header(&headers, "content-type") {
                let ct = if req.body.kind == BodyKind::Json {
                    "application/json"
                } else {
                    "text/plain"
                };
                headers.push(("Content-Type".into(), ct.into()));
            }
            Some(text.into_bytes())
        }
        BodyKind::Form => {
            let fields: Vec<String> = req
                .body
                .form
                .iter()
                .filter(|f| f.enabled && !f.key.trim().is_empty())
                .map(|f| {
                    format!(
                        "{}={}",
                        encode_component(&sub(&f.key), true),
                        encode_component(&sub(&f.value), true)
                    )
                })
                .collect();
            if !has_header(&headers, "content-type") {
                headers.push((
                    "Content-Type".into(),
                    "application/x-www-form-urlencoded".into(),
                ));
            }
            Some(fields.join("&").into_bytes())
        }
        BodyKind::Multipart => {
            let boundary = format!("rivet-{}", uuid::Uuid::new_v4().simple());
            let mut bytes = Vec::new();
            for field in req
                .body
                .form
                .iter()
                .filter(|f| f.enabled && !f.key.trim().is_empty())
            {
                bytes.extend_from_slice(
                    format!(
                        "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"",
                        disposition_value(&sub(&field.key))
                    )
                    .as_bytes(),
                );
                if field.file {
                    let path = sub(&field.value);
                    let file = std::path::Path::new(&path);
                    let filename = file.file_name().and_then(|s| s.to_str()).unwrap_or("file");
                    bytes.extend_from_slice(
                        format!(
                            "; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
                            disposition_value(filename),
                            file_mime(file)
                        )
                        .as_bytes(),
                    );
                    bytes.extend_from_slice(&read_body_file(&path)?);
                } else {
                    bytes.extend_from_slice(b"\r\n\r\n");
                    bytes.extend_from_slice(sub(&field.value).as_bytes());
                }
                bytes.extend_from_slice(b"\r\n");
            }
            bytes.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
            headers.retain(|(key, _)| !key.eq_ignore_ascii_case("content-type"));
            headers.push((
                "Content-Type".into(),
                format!("multipart/form-data; boundary={boundary}"),
            ));
            Some(bytes)
        }
        BodyKind::Binary => {
            let path = req.body.file_path.as_ref().ok_or_else(|| SendError::File {
                path: String::new(),
                message: "Выберите файл для бинарного тела".into(),
            })?;
            let path = sub(&path.to_string_lossy());
            if !has_header(&headers, "content-type") {
                headers.push((
                    "Content-Type".into(),
                    file_mime(std::path::Path::new(&path)).into(),
                ));
            }
            Some(read_body_file(&path)?)
        }
    };

    let bearer = |headers: &mut Vec<(String, String)>, token: String| {
        if !token.is_empty() && !has_header(headers, "authorization") {
            headers.push(("Authorization".into(), format!("Bearer {token}")));
        }
    };
    match &req.auth {
        Auth::None => {}
        Auth::Bearer { token } => {
            let t = sub(token.trim());
            bearer(&mut headers, t);
        }
        Auth::Inherit => {
            if let Some(token) = env.and_then(|e| e.var("token")) {
                let t = sub(token.trim());
                bearer(&mut headers, t);
            }
        }
        Auth::Basic { username, password } => {
            let creds = format!("{}:{}", sub(username), sub(password));
            if !has_header(&headers, "authorization") {
                headers.push((
                    "Authorization".into(),
                    format!("Basic {}", base64(creds.as_bytes())),
                ));
            }
        }
        Auth::ApiKey {
            key,
            value,
            in_query,
        } => {
            let (k, v) = (sub(key.trim()), sub(value));
            if !k.is_empty() {
                if *in_query {
                    query.push((k, v));
                } else if !has_header(&headers, &k) {
                    headers.push((k, v));
                }
            }
        }
    }

    append_query(&mut url, &query);

    Ok(ResolvedRequest {
        method: req.method,
        url,
        headers,
        body,
        missing_vars: missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{Body, EnvColor, KeyValue, Method};

    fn env() -> Environment {
        let mut e = Environment::new("Dev", EnvColor::Green);
        e.vars = vec![
            KeyValue::new("baseUrl", "http://localhost:8080"),
            KeyValue::new("id", "42"),
            KeyValue::new("off", "x").enabled(false),
            KeyValue::new("token", "tok123"),
        ];
        e
    }

    #[test]
    fn substitute_vars_and_missing() {
        let (s, m) = substitute(
            "{{baseUrl}}/a/{{ id }}/{{nope}}/{{off}}/{{nope}}",
            Some(&env()),
        );
        assert_eq!(s, "http://localhost:8080/a/42/{{nope}}/{{off}}/{{nope}}");
        assert_eq!(m, vec!["nope".to_string(), "off".to_string()]);
        assert_eq!(
            referenced_vars("{{a}} {{ b }} {{a}} {{$uuid}} {{}}"),
            vec!["a", "b"]
        );
    }

    #[test]
    fn nested_variables_and_cycles() {
        let mut e = env();
        e.vars.push(KeyValue::new("nested", "https://{{host}}/api"));
        e.vars.push(KeyValue::new("host", "example.test"));
        e.vars.push(KeyValue::new("cycle", "{{cycle}}"));
        assert_eq!(
            substitute("{{nested}}", Some(&e)),
            ("https://example.test/api".into(), vec![])
        );
        assert_eq!(
            substitute("{{cycle}}", Some(&e)),
            ("{{cycle}}".into(), vec!["cycle".into()])
        );
    }

    #[test]
    fn builtins() {
        let (s, m) = substitute("{{$timestamp}}|{{$uuid}}|{{$randomInt}}", None);
        assert!(m.is_empty());
        let parts: Vec<&str> = s.split('|').collect();
        assert!(parts[0].parse::<u64>().unwrap() > 1_600_000_000);
        assert_eq!(parts[1].len(), 36);
        assert!(parts[2].parse::<u32>().unwrap() <= 1000);
        let (_, m) = substitute("{{$bogus}}", None);
        assert_eq!(m, vec!["$bogus"]);
    }

    #[test]
    fn base64_works() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"user:pass"), "dXNlcjpwYXNz");
    }

    #[test]
    fn encode_decode() {
        assert_eq!(
            encode_component("a b&c=d/é", false),
            "a%20b%26c%3Dd%2F%C3%A9"
        );
        assert_eq!(encode_component("a b", true), "a+b");
        assert_eq!(decode_component("a%20b+c%zz%2", true), "a b c%zz%2");
    }

    #[test]
    fn resolve_url_rules() {
        let r = RequestSpec::new("x", Method::Get, "example.com");
        assert_eq!(resolve(&r, None).unwrap().url, "http://example.com/");
        let r = RequestSpec::new("x", Method::Get, "  ");
        assert!(matches!(resolve(&r, None), Err(SendError::InvalidUrl(_))));
        let r = RequestSpec::new("x", Method::Get, "ftp://x/");
        assert!(matches!(resolve(&r, None), Err(SendError::InvalidUrl(_))));
        let r = RequestSpec::new("x", Method::Get, "{{baseUrl}}/x");
        assert!(matches!(resolve(&r, None), Err(SendError::InvalidUrl(_))));
        let mut r = RequestSpec::new("x", Method::Get, "{{baseUrl}}/a b?x=1#frag");
        r.params = vec![
            KeyValue::new("q", "a&b c"),
            KeyValue::new("skip", "1").enabled(false),
        ];
        let res = resolve(&r, Some(&env())).unwrap();
        assert_eq!(res.url, "http://localhost:8080/a%20b?x=1&q=a%26b%20c");
    }

    #[test]
    fn resolve_body_and_auth() {
        let mut r = RequestSpec::new("x", Method::Post, "{{baseUrl}}/o");
        r.body = Body::json("{\"id\": {{id}}, \"m\": \"{{missing}}\"}");
        r.auth = Auth::Inherit;
        let res = resolve(&r, Some(&env())).unwrap();
        assert_eq!(
            res.body.as_deref(),
            Some(&b"{\"id\": 42, \"m\": \"{{missing}}\"}"[..])
        );
        assert_eq!(res.missing_vars, vec!["missing"]);
        assert!(
            res.headers
                .contains(&("Content-Type".into(), "application/json".into()))
        );
        assert!(
            res.headers
                .contains(&("Authorization".into(), "Bearer tok123".into()))
        );

        r.headers = vec![KeyValue::new("content-type", "application/vnd+json")];
        r.auth = Auth::ApiKey {
            key: "k".into(),
            value: "{{id}}".into(),
            in_query: true,
        };
        let res = resolve(&r, Some(&env())).unwrap();
        assert_eq!(
            res.headers
                .iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                .count(),
            1
        );
        assert_eq!(res.url, "http://localhost:8080/o?k=42");

        r.body = Body {
            kind: BodyKind::Form,
            text: String::new(),
            form: vec![KeyValue::new("a b", "c&d")],
            file_path: None,
        };
        r.headers.clear();
        r.auth = Auth::Basic {
            username: "user".into(),
            password: "pass".into(),
        };
        let res = resolve(&r, Some(&env())).unwrap();
        assert_eq!(res.body.as_deref(), Some(&b"a+b=c%26d"[..]));
        assert!(
            res.headers
                .contains(&("Authorization".into(), "Basic dXNlcjpwYXNz".into()))
        );
        assert!(res.headers.contains(&(
            "Content-Type".into(),
            "application/x-www-form-urlencoded".into()
        )));
    }
}
