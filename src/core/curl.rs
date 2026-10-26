//! See PLAN.md (core::curl).
//!
//! `parse_curl` understands the common subset of curl used in docs and
//! "Copy as cURL" from browsers; unknown flags are ignored. `to_curl` emits a
//! POSIX-shell command with single-quote escaping.

use super::types::{
    Auth, Body, BodyKind, Environment, KeyValue, Method, RequestSpec, ResolvedRequest,
};
use super::vars::decode_component;

/// Windows `cmd.exe` syntax ("Copy as cURL (cmd)"): `^` escapes the next
/// character and `^` at the end of a line continues the command. Turns it into
/// the POSIX form `tokenize` understands (`^\^"` → `\"`).
fn looks_like_cmd(cmd: &str) -> bool {
    cmd.contains("^\"") || cmd.lines().any(|l| l.trim_end().ends_with('^'))
}

fn unescape_cmd(cmd: &str) -> String {
    let mut out = String::with_capacity(cmd.len());
    let mut chars = cmd.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '^' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\r') => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push(' ');
            }
            Some('\n') => out.push(' '),
            Some(n) => out.push(n),
            None => {}
        }
    }
    out
}

fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

/// Pushes the character a `$'..'` escape stands for; `chars[*i]` is the char after `\`.
fn ansi_c_escape(chars: &[char], i: &mut usize, cur: &mut String) -> Option<()> {
    let c = *chars.get(*i)?;
    *i += 1;
    let hex = |i: &mut usize, max: usize| -> Option<char> {
        let start = *i;
        while *i < chars.len() && *i - start < max && chars[*i].is_ascii_hexdigit() {
            *i += 1;
        }
        let digits: String = chars[start..*i].iter().collect();
        u32::from_str_radix(&digits, 16).ok().and_then(char::from_u32)
    };
    let ch = match c {
        'n' => '\n',
        't' => '\t',
        'r' => '\r',
        'a' => '\u{7}',
        'b' => '\u{8}',
        'e' | 'E' => '\u{1b}',
        'f' => '\u{c}',
        'v' => '\u{b}',
        '0' => '\0',
        'x' => match hex(i, 2) {
            Some(ch) => ch,
            None => {
                cur.push_str("\\x");
                return Some(());
            }
        },
        'u' | 'U' => match hex(i, if c == 'u' { 4 } else { 8 }) {
            Some(ch) => ch,
            None => {
                cur.push('\\');
                cur.push(c);
                return Some(());
            }
        },
        other => other,
    };
    cur.push(ch);
    Some(())
}

/// Splits a shell command line into words (single/double quotes, `$'..'`,
/// backslash escapes, backslash-newline continuations). `None` on unbalanced quotes.
///
/// Continuations also survive a paste into a single-line field, which drops the
/// newlines: `\` followed by blanks that start a new word is a separator, not an
/// escaped space. A PowerShell backtick before a newline continues the line too.
fn tokenize(cmd: &str) -> Option<Vec<String>> {
    let unescaped;
    let cmd = if looks_like_cmd(cmd) {
        unescaped = unescape_cmd(cmd);
        unescaped.as_str()
    } else {
        cmd
    };
    let chars: Vec<char> = cmd.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        match c {
            '\\' => {
                // `\` + blanks + newline/end: a continuation with trailing spaces.
                let mut j = i;
                while j < chars.len() && is_blank(chars[j]) {
                    j += 1;
                }
                let blanks = j - i;
                match chars.get(j) {
                    Some('\n') | Some('\r') | None if blanks > 0 || j < chars.len() => {
                        i = j + 1;
                        if chars.get(j) == Some(&'\r') && chars.get(i) == Some(&'\n') {
                            i += 1;
                        }
                    }
                    None => {}
                    // A dropped newline: `\` then indentation before the next word.
                    _ if blanks > 0 && (!in_word || blanks > 1) => {
                        i = j;
                        if in_word {
                            out.push(std::mem::take(&mut cur));
                            in_word = false;
                        }
                    }
                    Some(_) => {
                        cur.push(chars[i]);
                        in_word = true;
                        i += 1;
                    }
                }
            }
            '`' if !in_word => {
                let mut j = i;
                while j < chars.len() && is_blank(chars[j]) {
                    j += 1;
                }
                match chars.get(j) {
                    // PowerShell continuation (or its newline dropped by a paste).
                    Some('\n') | Some('\r') | None => i = j,
                    _ if j > i => i = j,
                    _ => {
                        cur.push(c);
                        in_word = true;
                    }
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match *chars.get(i)? {
                        '\'' => {
                            i += 1;
                            break;
                        }
                        ch => {
                            cur.push(ch);
                            i += 1;
                        }
                    }
                }
            }
            '$' if chars.get(i) == Some(&'\'') => {
                i += 1;
                in_word = true;
                loop {
                    let ch = *chars.get(i)?;
                    i += 1;
                    match ch {
                        '\'' => break,
                        '\\' => ansi_c_escape(&chars, &mut i, &mut cur)?,
                        ch => cur.push(ch),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    let ch = *chars.get(i)?;
                    i += 1;
                    match ch {
                        '"' => break,
                        '\\' => {
                            let next = *chars.get(i)?;
                            i += 1;
                            match next {
                                '\n' => {}
                                '\r' if chars.get(i) == Some(&'\n') => i += 1,
                                ch @ ('"' | '\\' | '$' | '`') => cur.push(ch),
                                ch => {
                                    cur.push('\\');
                                    cur.push(ch);
                                }
                            }
                        }
                        ch => cur.push(ch),
                    }
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        out.push(cur);
    }
    Some(out)
}

/// Unknown flags that consume the next word (so it is not mistaken for the URL).
const FLAGS_WITH_VALUE: &[&str] = &[
    "-o",
    "--output",
    "-w",
    "--write-out",
    "-m",
    "--max-time",
    "--connect-timeout",
    "-x",
    "--proxy",
    "--cacert",
    "--capath",
    "-E",
    "--cert",
    "--key",
    "--cert-type",
    "--key-type",
    "-F",
    "--form",
    "--form-string",
    "-T",
    "--upload-file",
    "--retry",
    "--retry-delay",
    "--retry-max-time",
    "-c",
    "--cookie-jar",
    "--resolve",
    "-r",
    "--range",
    "-K",
    "--config",
    "--limit-rate",
    "-U",
    "--proxy-user",
    "--max-redirs",
    "--interface",
    "-Y",
    "--speed-limit",
    "-y",
    "--speed-time",
    "--ciphers",
    "--dns-servers",
    "--connect-to",
    "-z",
    "--time-cond",
    "--max-filesize",
    "--keepalive-time",
    "--unix-socket",
    "--abstract-unix-socket",
    "--trace",
    "--trace-ascii",
    "--stderr",
    "-D",
    "--dump-header",
    "--oauth2-bearer",
    "--aws-sigv4",
    "-Q",
    "--quote",
    "--local-port",
];

/// Splits a raw query string into params (percent-decoded, `+` as space).
fn split_query(q: &str) -> Vec<KeyValue> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            KeyValue::new(decode_component(k, true), decode_component(v, true))
        })
        .collect()
}

fn looks_like_form(data: &str) -> bool {
    !data.is_empty()
        && !data.contains(['\n', '{', '['])
        && data.split('&').all(|p| {
            p.split_once('=')
                .is_some_and(|(k, _)| !k.is_empty() && !k.contains(' '))
        })
}

fn name_from_url(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let path = rest.find('/').map_or("", |i| &rest[i..]);
    let path = path.split(['?', '#']).next().unwrap_or("");
    if path.is_empty() || path == "/" {
        rest.split(['/', '?', '#'])
            .next()
            .unwrap_or("cURL import")
            .to_string()
    } else {
        path.to_string()
    }
}

/// Cleans up a command copied from a web page or docs: a leading shell prompt
/// (`$ curl …`), non-breaking / typographic spaces and “smart” quotes.
fn normalize_pasted(cmd: &str) -> String {
    let cmd: String = cmd
        .chars()
        .map(|c| match c {
            '\u{a0}' | '\u{2007}' | '\u{2009}' | '\u{202f}' | '\u{3000}' => ' ',
            '\u{201c}' | '\u{201d}' | '\u{201e}' => '"',
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{feff}' | '\u{200b}' => ' ',
            c => c,
        })
        .collect();
    let trimmed = cmd.trim();
    let trimmed = ["$ ", "% ", "> ", "# "]
        .iter()
        .find_map(|p| trimmed.strip_prefix(p))
        .unwrap_or(trimmed);
    trimmed.trim_start().to_string()
}

/// Whether pasted text is a curl command (`curl …`, `$ curl …`, `curl.exe …`).
pub fn looks_like_curl(text: &str) -> bool {
    let text = normalize_pasted(text);
    let first = text.split(|c: char| c.is_whitespace() || c == '\\').next().unwrap_or("");
    let prog = first.rsplit('/').next().unwrap_or(first);
    !first.contains("://")
        && (prog.eq_ignore_ascii_case("curl") || prog.eq_ignore_ascii_case("curl.exe"))
}

/// Parses a `curl ...` command into a request. `None` if it isn't a curl command
/// or has no URL.
pub fn parse_curl(cmd: &str) -> Option<RequestSpec> {
    let words = tokenize(&normalize_pasted(cmd))?;
    let mut it = words.into_iter();
    let first = it.next()?;
    let prog = first.rsplit('/').next().unwrap_or(&first);
    if !prog.eq_ignore_ascii_case("curl") && !prog.eq_ignore_ascii_case("curl.exe") {
        return None;
    }

    let mut method: Option<Method> = None;
    let mut url: Option<String> = None;
    let mut headers: Vec<KeyValue> = Vec::new();
    let mut data: Vec<String> = Vec::new();
    let mut json_flag = false;
    let mut get = false;
    let mut head = false;
    let mut user: Option<String> = None;

    let words: Vec<String> = it.collect();
    let mut i = 0;
    while i < words.len() {
        let w = &words[i];
        i += 1;
        // `--flag=value` and `-Xvalue` forms.
        let (flag, inline): (String, Option<String>) = if w.starts_with("--") {
            match w.split_once('=') {
                Some((f, v)) => (f.to_string(), Some(v.to_string())),
                None => (w.clone(), None),
            }
        } else if w.starts_with('-')
            && w.len() > 2
            && matches!(&w[..2], "-X" | "-H" | "-d" | "-u" | "-A" | "-b" | "-e")
        {
            (w[..2].to_string(), Some(w[2..].to_string()))
        } else {
            (w.clone(), None)
        };
        let value = |i: &mut usize| -> Option<String> {
            if let Some(v) = inline.clone() {
                return Some(v);
            }
            let v = words.get(*i).cloned();
            *i += 1;
            v
        };
        match flag.as_str() {
            "-X" | "--request" => {
                method = value(&mut i)
                    .and_then(|m| Method::parse(m.trim()))
                    .or(method)
            }
            "-H" | "--header" => {
                if let Some(h) = value(&mut i) {
                    if let Some((k, v)) = h.split_once(':') {
                        headers.push(KeyValue::new(k.trim(), v.trim()));
                    }
                }
            }
            "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-ascii"
            | "--data-urlencode" => {
                if let Some(d) = value(&mut i) {
                    data.push(d);
                }
            }
            "--json" => {
                if let Some(d) = value(&mut i) {
                    data.push(d);
                    json_flag = true;
                }
            }
            "-u" | "--user" => user = value(&mut i),
            "--url" => url = value(&mut i).or(url),
            "-A" | "--user-agent" => {
                if let Some(v) = value(&mut i) {
                    headers.push(KeyValue::new("User-Agent", v));
                }
            }
            "-b" | "--cookie" => {
                if let Some(v) = value(&mut i) {
                    headers.push(KeyValue::new("Cookie", v));
                }
            }
            "-e" | "--referer" => {
                if let Some(v) = value(&mut i) {
                    headers.push(KeyValue::new("Referer", v));
                }
            }
            "-G" | "--get" => get = true,
            "-I" | "--head" => head = true,
            f if FLAGS_WITH_VALUE.contains(&f) => {
                let _ = value(&mut i);
            }
            f if f.starts_with('-') && f.len() > 1 => {}
            _ => {
                if url.is_none() {
                    url = Some(w.clone());
                }
            }
        }
    }

    let url = url?.trim().to_string();
    if url.is_empty() {
        return None;
    }
    let (base, query) = match url.split_once('?') {
        Some((b, q)) => (b.to_string(), q.split('#').next().unwrap_or("").to_string()),
        None => (url.clone(), String::new()),
    };
    let mut spec = RequestSpec::new(name_from_url(&url), Method::Get, base);
    spec.params = split_query(&query);

    let joined = data.join("&");
    let content_type = headers
        .iter()
        .find(|h| h.key.eq_ignore_ascii_case("content-type"))
        .map(|h| h.value.to_ascii_lowercase());
    if json_flag {
        if content_type.is_none() {
            headers.push(KeyValue::new("Content-Type", "application/json"));
        }
        if !headers.iter().any(|h| h.key.eq_ignore_ascii_case("accept")) {
            headers.push(KeyValue::new("Accept", "application/json"));
        }
    }

    if !data.is_empty() && get {
        spec.params.extend(split_query(&joined));
    } else if !data.is_empty() {
        let trimmed = joined.trim_start();
        let is_json = json_flag
            || content_type.as_deref().is_some_and(|c| c.contains("json"))
            || ((trimmed.starts_with('{') || trimmed.starts_with('['))
                && serde_json::from_str::<serde_json::Value>(&joined).is_ok());
        let is_form_ct = content_type
            .as_deref()
            .is_some_and(|c| c.contains("x-www-form-urlencoded"));
        spec.body = if is_json {
            Body::json(joined)
        } else if (content_type.is_none() || is_form_ct) && looks_like_form(&joined) {
            Body {
                kind: BodyKind::Form,
                text: String::new(),
                form: split_query(&joined),
                file_path: None,
                ..Body::default()
            }
        } else {
            if content_type.is_none() {
                // What curl itself would send for -d.
                headers.push(KeyValue::new(
                    "Content-Type",
                    "application/x-www-form-urlencoded",
                ));
            }
            Body {
                kind: BodyKind::Text,
                text: joined,
                form: Vec::new(),
                file_path: None,
                ..Body::default()
            }
        };
    }

    // `Authorization: Bearer x` becomes the Auth tab.
    if let Some(pos) = headers.iter().position(|h| {
        h.key.eq_ignore_ascii_case("authorization")
            && h.value.len() > 7
            && h.value[..7].eq_ignore_ascii_case("bearer ")
    }) {
        let h = headers.remove(pos);
        spec.auth = Auth::Bearer {
            token: h.value[7..].trim().to_string(),
        };
    }
    if let Some(u) = user {
        let (username, password) = u.split_once(':').unwrap_or((&u, ""));
        spec.auth = Auth::Basic {
            username: username.to_string(),
            password: password.to_string(),
        };
    }
    spec.headers = headers;

    spec.method = match (method, head) {
        (Some(m), _) => m,
        (None, true) => Method::Head,
        (None, false) if !data.is_empty() && !get => Method::Post,
        _ => Method::Get,
    };
    Some(spec)
}

/// Single-quotes a word for POSIX shells: `it's` → `'it'\''s'`.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// A copy-pasteable curl command for the resolved request.
pub fn to_curl(req: &ResolvedRequest) -> String {
    let mut parts: Vec<String> = Vec::new();
    match req.method {
        Method::Get => parts.push(format!("curl {}", quote(&req.url))),
        Method::Head => parts.push(format!("curl --head {}", quote(&req.url))),
        m => parts.push(format!("curl -X {} {}", m.full_label(), quote(&req.url))),
    }
    for (k, v) in &req.headers {
        parts.push(format!("-H {}", quote(&format!("{k}: {v}"))));
    }
    if let Some(body) = &req.body {
        if req.method != Method::Head {
            parts.push(format!(
                "--data-raw {}",
                quote(&String::from_utf8_lossy(body))
            ));
        }
    }
    parts.join(" \\\n  ")
}

/// Preserve file references and multipart fields when copying a request as cURL.
pub fn to_curl_with_source(
    source: &RequestSpec,
    req: &ResolvedRequest,
    env: Option<&Environment>,
) -> String {
    if !matches!(source.body.kind, BodyKind::Multipart | BodyKind::Binary) {
        return to_curl(req);
    }
    let mut parts = match req.method {
        Method::Get => vec![format!("curl {}", quote(&req.url))],
        Method::Head => vec![format!("curl --head {}", quote(&req.url))],
        method => vec![format!(
            "curl -X {} {}",
            method.full_label(),
            quote(&req.url)
        )],
    };
    for (key, value) in &req.headers {
        if source.body.kind == BodyKind::Multipart && key.eq_ignore_ascii_case("content-type") {
            // cURL supplies its own multipart boundary.
            continue;
        }
        parts.push(format!("-H {}", quote(&format!("{key}: {value}"))));
    }
    match source.body.kind {
        BodyKind::Binary => {
            if let Some(path) = &source.body.file_path {
                let path = super::vars::substitute(&path.to_string_lossy(), env).0;
                parts.push(format!("--data-binary {}", quote(&format!("@{path}"))));
            }
        }
        BodyKind::Multipart => {
            for field in source
                .body
                .form
                .iter()
                .filter(|f| f.enabled && !f.key.trim().is_empty())
            {
                let key = super::vars::substitute(&field.key, env).0;
                let value = super::vars::substitute(&field.value, env).0;
                let arg = if field.file {
                    format!("{key}=@{value}")
                } else {
                    format!("{key}={value}")
                };
                parts.push(format!("-F {}", quote(&arg)));
            }
        }
        _ => {}
    }
    parts.join(" \\\n  ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer() {
        let t = tokenize("curl 'a b' \"c \\\"d\\\"\" e\\ f \\\n  g $'x\\ny'").unwrap();
        assert_eq!(t, vec!["curl", "a b", "c \"d\"", "e f", "g", "x\ny"]);
        assert!(tokenize("curl 'oops").is_none());
    }

    #[test]
    fn continuations_and_escapes() {
        let words = vec!["curl", "-X", "POST", "https://h/x", "-H", "A: b"];
        // Classic, CRLF and trailing blanks after the backslash.
        for cmd in [
            "curl -X POST \\\n  'https://h/x' \\\n  -H 'A: b'",
            "curl -X POST \\\r\n  'https://h/x' \\\r\n  -H 'A: b'",
            "curl -X POST \\  \n  'https://h/x' \\\t\n  -H 'A: b' \\",
            // Newlines dropped by a single-line input.
            "curl -X POST \\  'https://h/x' \\  -H 'A: b'",
            "curl -X POST\\  'https://h/x'\\  -H 'A: b'",
            // PowerShell backtick continuation.
            "curl -X POST `\n  'https://h/x' `\n  -H 'A: b'",
            // Windows cmd: `^` continuation and `^"` quoting.
            "curl -X POST ^\n  ^\"https://h/x^\" ^\r\n  -H ^\"A: b^\"",
        ] {
            assert_eq!(tokenize(cmd).unwrap(), words, "{cmd:?}");
        }
        // Escaped characters outside quotes and in $'..'.
        let t = tokenize(r#"curl a\$b \"q\" x\\y $'\x41\u00e9\'z'"#).unwrap();
        assert_eq!(t, vec!["curl", "a$b", "\"q\"", "x\\y", "Aé'z"]);
        // cmd JSON body: `^\^"` is an escaped quote inside a quoted word.
        let s = parse_curl(
            "curl ^\"https://h/x^\" ^\n  --data-raw ^\"^{^\\^\"a^\\^\":1^}^\"",
        )
        .unwrap();
        assert_eq!(s.body, Body::json("{\"a\":1}"));
    }

    #[test]
    fn pasted_from_web_pages() {
        let cmd = "$ curl -L \\\n  -H \u{201c}Accept: application/json\u{201d} \\\n\u{a0}\u{a0}https://api.github.com/repos/o/r";
        assert!(looks_like_curl(cmd));
        let s = parse_curl(cmd).unwrap();
        assert_eq!(s.url, "https://api.github.com/repos/o/r");
        assert_eq!(s.headers, vec![KeyValue::new("Accept", "application/json")]);
        assert!(looks_like_curl("curl\\\n  https://x"));
        assert!(!looks_like_curl("curly http://x"));
        assert!(!looks_like_curl("https://x/curl"));
    }

    #[test]
    fn parse_multiline_after_newlines_dropped() {
        let raw = "curl 'https://api.x.com/v1/users?limit=5' \\\n  -H 'Accept: application/json' \\\n  -H 'Authorization: Bearer t0k' \\\n  --data-raw '{\"a\":1}'";
        let flat = raw.replace('\n', "");
        for cmd in [raw.to_string(), flat] {
            let s = parse_curl(&cmd).unwrap();
            assert_eq!(s.url, "https://api.x.com/v1/users", "{cmd:?}");
            assert_eq!(s.params, vec![KeyValue::new("limit", "5")]);
            assert_eq!(s.headers, vec![KeyValue::new("Accept", "application/json")]);
            assert_eq!(s.auth, Auth::Bearer { token: "t0k".into() });
            assert_eq!(s.body, Body::json("{\"a\":1}"));
            assert_eq!(s.method, Method::Post);
        }
    }

    #[test]
    fn parse_basic() {
        let s = parse_curl(
            "curl -X PUT 'https://api.x.com/orders/1?expand=items&q=a%20b' \\\n  -H 'Content-Type: application/json' \\\n  -H \"Authorization: Bearer abc\" \\\n  --data-raw '{\"qty\": 2}' -sSL --compressed -o out.txt",
        )
        .unwrap();
        assert_eq!(s.method, Method::Put);
        assert_eq!(s.url, "https://api.x.com/orders/1");
        assert_eq!(
            s.params,
            vec![KeyValue::new("expand", "items"), KeyValue::new("q", "a b")]
        );
        assert_eq!(
            s.headers,
            vec![KeyValue::new("Content-Type", "application/json")]
        );
        assert_eq!(
            s.auth,
            Auth::Bearer {
                token: "abc".into()
            }
        );
        assert_eq!(s.body, Body::json("{\"qty\": 2}"));
    }

    #[test]
    fn parse_data_implies_post_and_user() {
        let s = parse_curl("curl example.com/login -d 'user=a b&x=1' -u bob:secret").unwrap();
        assert_eq!(s.method, Method::Post);
        assert_eq!(s.body.kind, BodyKind::Form);
        assert_eq!(
            s.body.form,
            vec![KeyValue::new("user", "a b"), KeyValue::new("x", "1")]
        );
        assert_eq!(
            s.auth,
            Auth::Basic {
                username: "bob".into(),
                password: "secret".into()
            }
        );
        let s = parse_curl("curl --url=http://h/x -d '{\"a\":1}'").unwrap();
        assert_eq!(s.body.kind, BodyKind::Json);
        assert!(parse_curl("wget http://x").is_none());
        assert!(parse_curl("curl -sS").is_none());
        let s = parse_curl("curl -I http://h/").unwrap();
        assert_eq!(s.method, Method::Head);
    }

    #[test]
    fn quoting() {
        let r = ResolvedRequest {
            method: Method::Post,
            url: "http://h/x".into(),
            headers: vec![("X-Q".into(), "it's".into())],
            body: Some(b"{\"a\":\"b'c\"}".to_vec()),
            missing_vars: vec![],
        };
        let c = to_curl(&r);
        assert_eq!(
            c,
            "curl -X POST 'http://h/x' \\\n  -H 'X-Q: it'\\''s' \\\n  --data-raw '{\"a\":\"b'\\''c\"}'"
        );
        let back = parse_curl(&c).unwrap();
        assert_eq!(back.headers, vec![KeyValue::new("X-Q", "it's")]);
        assert_eq!(back.body.text, "{\"a\":\"b'c\"}");
    }
}
