//! core::vars + core::http + core::curl against a tiny local HTTP/1.1 server,
//! plus hyper-based h2c / TLS (ALPN h2) servers and a CONNECT proxy.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use rivet::core::curl::{parse_curl, to_curl};
use rivet::core::http::{
    CancelToken, HttpSessions, UNSAVED_COOKIE_SCOPE, execute, execute_scoped, join_url,
};
use rivet::core::vars::resolve;
use rivet::core::{
    Auth, Body, EnvColor, Environment, HttpVersionPref, KeyValue, Method, RequestSpec, SendError,
    Settings,
};
use serde_json::Value;

/// Echo server: `/echo*` returns the request as JSON, `/missing` → 404,
/// `/sleep` waits 3 s before answering, `/start`, `/next`, `/see-other`,
/// `/temporary`, `/rel/a/b`, `/redirect*` redirect. Returns the base URL.
fn server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            std::thread::spawn(move || handle(stream));
        }
    });
    format!("http://{addr}")
}

fn handle(stream: TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let mut headers = Vec::new();
    let mut len = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).unwrap_or(0) == 0 {
            break;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                len = v.trim().parse().unwrap_or(0);
            }
            headers.push((k.to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    let mut body = vec![0u8; len];
    let _ = reader.read_exact(&mut body);

    let mut stream = stream;
    let path = target.split('?').next().unwrap_or("");
    if path == "/redirect" || path == "/redirect-loop" {
        let location = if path == "/redirect" {
            "/echo"
        } else {
            "/redirect-loop"
        };
        let head = format!(
            "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let _ = stream.write_all(head.as_bytes());
        return;
    }
    // Redirect chain: /start → 302 (absolute) → /next → 301 → /final (echo).
    let host = headers
        .iter()
        .find(|(k, _)| k == "host")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let redirect = match path {
        "/start" => Some(("302 Found", format!("http://{host}/next"))),
        "/next" => Some(("301 Moved Permanently", "/final".to_string())),
        "/see-other" => Some(("303 See Other", "/echo".to_string())),
        "/temporary" => Some(("307 Temporary Redirect", "/echo".to_string())),
        "/rel/a/b" => Some(("302 Found", "../c?x=1".to_string())),
        _ => None,
    };
    if let Some((status, location)) = redirect {
        let head = format!(
            "HTTP/1.1 {status}\r\nLocation: {location}\r\nX-Hop: {path}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let _ = stream.write_all(head.as_bytes());
        return;
    }
    if path == "/cookie/redirect" {
        let _ = stream.write_all(b"HTTP/1.1 302 Found\r\nSet-Cookie: hop=1; Path=/\r\nLocation: /echo\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }
    if path == "/gzip" {
        use flate2::write::GzEncoder;
        let mut enc = GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(br#"{"zipped":true}"#).unwrap();
        let gz = enc.finish().unwrap();
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nX-Accept-Encoding: {}\r\nConnection: close\r\n\r\n",
            gz.len(),
            headers
                .iter()
                .find(|(k, _)| k == "accept-encoding")
                .map(|(_, v)| v.as_str())
                .unwrap_or("")
        );
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(&gz);
        return;
    }
    if path == "/cookie/set" {
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nSet-Cookie: session=alpha; Path=/\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
        return;
    }
    let (status, payload) = match path {
        "/missing" => ("404 Not Found", r#"{"error":"not found"}"#.to_string()),
        "/sleep" => {
            std::thread::sleep(Duration::from_secs(3));
            ("200 OK", "{}".to_string())
        }
        _ => {
            let hdrs: serde_json::Map<String, Value> = headers
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect();
            let v = serde_json::json!({
                "method": method,
                "target": target,
                "headers": hdrs,
                "body": String::from_utf8_lossy(&body),
            });
            ("200 OK", v.to_string())
        }
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nX-Multi: a\r\nX-Multi: b\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    let _ = stream.write_all(head.as_bytes());
    if method != "HEAD" {
        let _ = stream.write_all(payload.as_bytes());
    }
    let _ = stream.flush();
}

#[test]
fn redirects_can_be_disabled_and_limited() {
    let base = server();
    let req = resolve(
        &RequestSpec::new("r", Method::Get, format!("{base}/redirect")),
        None,
    )
    .unwrap();
    let followed = execute(&req, &Settings::default(), &CancelToken::default()).unwrap();
    assert_eq!(followed.status, 200);
    assert_eq!(followed.redirect_history.len(), 2);

    let settings = Settings {
        follow_redirects: false,
        ..Settings::default()
    };
    let stopped = execute(&req, &settings, &CancelToken::default()).unwrap();
    assert_eq!(stopped.status, 302);

    let loop_req = resolve(
        &RequestSpec::new("loop", Method::Get, format!("{base}/redirect-loop")),
        None,
    )
    .unwrap();
    let settings = Settings {
        max_redirects: 2,
        ..Settings::default()
    };
    // Limit reached: the last 3xx is returned with the hops followed so far.
    let limited = execute(&loop_req, &settings, &CancelToken::default()).unwrap();
    assert_eq!(limited.status, 302);
    assert_eq!(limited.hops.len(), 2);
    assert_eq!(limited.redirect_history.len(), 3);
}

fn get(url: String, settings: &Settings) -> rivet::core::ResponseData {
    let req = resolve(&RequestSpec::new("r", Method::Get, url), None).unwrap();
    execute(&req, settings, &CancelToken::default()).unwrap()
}

#[test]
fn redirect_chain_records_every_hop() {
    let base = server();
    let resp = get(format!("{base}/start"), &Settings::default());
    assert_eq!(resp.status, 200);
    let json: Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(json["target"], "/final");

    assert_eq!(resp.hops.len(), 2);
    let (a, b) = (&resp.hops[0], &resp.hops[1]);
    assert_eq!(a.method, Method::Get);
    assert_eq!(a.url, format!("{base}/start"));
    assert_eq!(a.status, 302);
    assert_eq!(a.status_text, "Found");
    assert_eq!(a.location, format!("{base}/next"));
    assert!(a.headers.iter().any(|(k, v)| k == "x-hop" && v == "/start"));
    assert!(a.remote_addr.as_deref().unwrap().starts_with("127.0.0.1:"));
    assert_eq!(b.url, format!("{base}/next"));
    assert_eq!(b.status, 301);
    assert_eq!(b.location, format!("{base}/final"));
    assert_eq!(
        resp.redirect_history,
        vec![
            format!("{base}/start"),
            format!("{base}/next"),
            format!("{base}/final")
        ]
    );

    let t = &resp.timings;
    assert!(a.timings.total > Duration::ZERO);
    assert_eq!(t.redirect, a.timings.total + b.timings.total);
    assert!(t.redirect > Duration::ZERO);
    assert!(t.redirect <= t.total);
    assert!(t.redirect + t.dns + t.connect + t.tls + t.ttfb + t.download <= t.total);
}

#[test]
fn see_other_after_post_becomes_bodyless_get() {
    let base = server();
    let mut spec = RequestSpec::new("p", Method::Post, format!("{base}/see-other"));
    spec.body = Body::json(r#"{"a":1}"#);
    let (resp, json) = send(&spec, None);
    assert_eq!(resp.hops.len(), 1);
    assert_eq!(resp.hops[0].method, Method::Post);
    assert_eq!(resp.hops[0].status, 303);
    assert_eq!(json["method"], "GET");
    assert_eq!(json["body"], "");
    assert!(json["headers"].get("content-type").is_none());
    assert!(json["headers"].get("content-length").is_none());

    // 302 after POST also switches to GET (browser behaviour).
    spec.url = format!("{base}/redirect");
    let (_, json) = send(&spec, None);
    assert_eq!(json["method"], "GET");
    assert_eq!(json["body"], "");
}

#[test]
fn temporary_redirect_keeps_method_and_body() {
    let base = server();
    let mut spec = RequestSpec::new("p", Method::Post, format!("{base}/temporary"));
    spec.body = Body::json(r#"{"a":1}"#);
    let (resp, json) = send(&spec, None);
    assert_eq!(resp.hops.len(), 1);
    assert_eq!(resp.hops[0].status, 307);
    assert_eq!(json["method"], "POST");
    assert_eq!(json["body"], r#"{"a":1}"#);
    assert_eq!(json["headers"]["content-type"], "application/json");
}

#[test]
fn relative_location_is_resolved() {
    let base = server();
    let resp = get(format!("{base}/rel/a/b"), &Settings::default());
    assert_eq!(resp.hops[0].location, format!("{base}/rel/c?x=1"));
    let json: Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(json["target"], "/rel/c?x=1");
}

#[test]
fn disabled_redirects_return_the_3xx_without_hops() {
    let base = server();
    let settings = Settings {
        follow_redirects: false,
        ..Settings::default()
    };
    let resp = get(format!("{base}/start"), &settings);
    assert_eq!(resp.status, 302);
    assert_eq!(resp.header("location"), Some(format!("{base}/next").as_str()));
    assert!(resp.hops.is_empty());
    assert_eq!(resp.redirect_history, vec![format!("{base}/start")]);
    assert_eq!(resp.timings.redirect, Duration::ZERO);

    let settings = Settings {
        max_redirects: 1,
        ..Settings::default()
    };
    let resp = get(format!("{base}/start"), &settings);
    assert_eq!(resp.status, 301);
    assert_eq!(resp.hops.len(), 1);
}

#[test]
fn join_url_cases() {
    let base = "http://h.com:8080/a/b/c?q=1#f";
    assert_eq!(join_url(base, "https://x.org/y"), "https://x.org/y");
    assert_eq!(join_url(base, "//cdn.x/z"), "http://cdn.x/z");
    assert_eq!(join_url(base, "/root?k=v"), "http://h.com:8080/root?k=v");
    assert_eq!(join_url(base, "d"), "http://h.com:8080/a/b/d");
    assert_eq!(join_url(base, "../d/./e"), "http://h.com:8080/a/d/e");
    assert_eq!(join_url(base, "../../../../d"), "http://h.com:8080/d");
    assert_eq!(join_url(base, "?z=2"), "http://h.com:8080/a/b/c?z=2");
    assert_eq!(join_url(base, "#top"), "http://h.com:8080/a/b/c?q=1#top");
    assert_eq!(join_url("http://h.com", "x"), "http://h.com/x");
    assert_eq!(join_url("http://h.com", "?a"), "http://h.com/?a");
    assert_eq!(join_url("http://h.com/dir/", "."), "http://h.com/dir/");
    assert_eq!(join_url("http://h.com/a/b", ".."), "http://h.com/");
    assert_eq!(join_url("https://h.com/a", " /b "), "https://h.com/b");
}

#[test]
fn cookies_are_scoped_to_collection_and_can_be_cleared() {
    let base = server();
    let set = resolve(
        &RequestSpec::new("set", Method::Get, format!("{base}/cookie/set")),
        None,
    )
    .unwrap();
    let echo = resolve(
        &RequestSpec::new("echo", Method::Get, format!("{base}/echo")),
        None,
    )
    .unwrap();
    let sessions = HttpSessions::default();
    let settings = Settings::default();
    let cancel = CancelToken::default();
    execute_scoped(&set, &settings, &cancel, &sessions, Some("a")).unwrap();
    let read = |scope: Option<&str>| {
        let response = execute_scoped(&echo, &settings, &cancel, &sessions, scope).unwrap();
        let json: Value = serde_json::from_slice(&response.body).unwrap();
        json["headers"]
            .get("cookie")
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    assert!(
        read(Some("a"))
            .as_deref()
            .is_some_and(|header| header.starts_with("session=alpha"))
    );
    assert_eq!(read(Some("b")), None);
    assert_eq!(read(None), None);
    execute_scoped(
        &set,
        &settings,
        &cancel,
        &sessions,
        Some(UNSAVED_COOKIE_SCOPE),
    )
    .unwrap();
    assert!(read(Some(UNSAVED_COOKIE_SCOPE)).is_some());
    sessions.clear("a");
    assert_eq!(read(Some("a")), None);
    sessions.clear_all();
    assert_eq!(read(Some(UNSAVED_COOKIE_SCOPE)), None);
}

fn env(base: &str) -> Environment {
    let mut e = Environment::new("Dev", EnvColor::Green);
    e.vars = vec![
        KeyValue::new("baseUrl", base),
        KeyValue::new("id", "42"),
        KeyValue::new("token", "envtok"),
    ];
    e
}

fn send(spec: &RequestSpec, env: Option<&Environment>) -> (rivet::core::ResponseData, Value) {
    let req = resolve(spec, env).unwrap();
    let resp = execute(&req, &Settings::default(), &CancelToken::default()).unwrap();
    let json = serde_json::from_slice(&resp.body).unwrap_or(Value::Null);
    (resp, json)
}

#[test]
fn get_with_headers() {
    let base = server();
    let mut spec = RequestSpec::new("get", Method::Get, format!("{base}/echo"));
    spec.headers = vec![
        KeyValue::new("X-Trace", "abc"),
        KeyValue::new("X-Off", "1").enabled(false),
    ];
    let (resp, json) = send(&spec, None);
    assert_eq!(resp.status, 200);
    assert_eq!(resp.status_text, "OK");
    assert_eq!(resp.http_version, "HTTP/1.1");
    assert_eq!(json["method"], "GET");
    assert_eq!(json["headers"]["x-trace"], "abc");
    assert!(json["headers"].get("x-off").is_none());
    assert!(
        resp.remote_addr
            .as_deref()
            .unwrap()
            .starts_with("127.0.0.1:")
    );
    let multi: Vec<&str> = resp
        .headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("x-multi"))
        .map(|(_, v)| v.as_str())
        .collect();
    assert_eq!(multi, vec!["a", "b"]);
    assert!(resp.timings.total >= resp.timings.ttfb);
    assert!(resp.timings.total > Duration::ZERO);
}

#[test]
fn post_json_body() {
    let base = server();
    let mut spec = RequestSpec::new("post", Method::Post, "{{baseUrl}}/echo");
    spec.body = Body::json(r#"{"id": {{id}}}"#);
    let (resp, json) = send(&spec, Some(&env(&base)));
    assert_eq!(resp.status, 200);
    assert_eq!(json["method"], "POST");
    assert_eq!(json["body"], r#"{"id": 42}"#);
    assert_eq!(json["headers"]["content-type"], "application/json");
}

#[test]
fn switched_off_auto_headers_are_not_sent() {
    use rivet::core::auto_headers::set_enabled;
    let base = server();
    let mut spec = RequestSpec::new("post", Method::Post, format!("{base}/echo"));
    spec.body = Body::json("{}");
    spec.auth = Auth::Bearer { token: "tok".into() };
    let (_, json) = send(&spec, None);
    let sent = &json["headers"];
    assert_eq!(sent["user-agent"], concat!("rivet/", env!("CARGO_PKG_VERSION")));
    assert_eq!(sent["accept"], "*/*");
    assert_eq!(sent["authorization"], "Bearer tok");
    assert_eq!(sent["content-type"], "application/json");

    for name in ["User-Agent", "Accept", "Authorization", "Content-Type", "Host"] {
        set_enabled(&mut spec, name, false);
    }
    let (resp, json) = send(&spec, None);
    assert_eq!(resp.status, 200);
    let sent = &json["headers"];
    for name in ["user-agent", "accept", "authorization", "content-type"] {
        assert!(sent.get(name).is_none(), "{name} was sent: {sent}");
    }
    // Host and Content-Length can't be switched off; Accept-Encoding stayed on.
    assert!(sent.get("host").is_some());
    assert_eq!(sent["content-length"], "2");
    assert!(sent.get("accept-encoding").is_some());

    // A header set by hand is sent even when the auto one is off.
    spec.headers = vec![KeyValue::new("Accept", "application/json")];
    let (_, json) = send(&spec, None);
    assert_eq!(json["headers"]["accept"], "application/json");
}

#[test]
fn multipart_form_data_uploads_selected_file() {
    let base = server();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("upload.txt");
    std::fs::write(&file, "file payload").unwrap();
    let mut attachment = KeyValue::new("attachment", file.display().to_string());
    attachment.file = true;
    let mut spec = RequestSpec::new("upload", Method::Post, format!("{base}/echo"));
    spec.body = Body {
        kind: rivet::core::BodyKind::Multipart,
        form: vec![KeyValue::new("note", "hello"), attachment],
        ..Body::default()
    };

    let (_, echoed) = send(&spec, None);
    let content_type = echoed["headers"]["content-type"].as_str().unwrap();
    assert!(content_type.starts_with("multipart/form-data; boundary=rivet-"));
    let body = echoed["body"].as_str().unwrap();
    assert!(body.contains("name=\"note\"\r\n\r\nhello"));
    assert!(body.contains("name=\"attachment\"; filename=\"upload.txt\""));
    assert!(body.contains("Content-Type: text/plain\r\n\r\nfile payload"));
    assert!(body.ends_with(&format!(
        "--{}--\r\n",
        content_type.split("boundary=").nth(1).unwrap()
    )));
}

#[test]
fn delete_with_body_and_options() {
    let base = server();
    let mut spec = RequestSpec::new("del", Method::Delete, format!("{base}/echo"));
    spec.body = Body {
        kind: rivet::core::BodyKind::Text,
        text: "bye".into(),
        form: vec![],
        file_path: None,
        variables: String::new(),
    };
    let (_, json) = send(&spec, None);
    assert_eq!(json["method"], "DELETE");
    assert_eq!(json["body"], "bye");
    assert_eq!(json["headers"]["content-type"], "text/plain");

    let mut gql = RequestSpec::new("gql", Method::Post, format!("{base}/echo"));
    gql.body = Body {
        kind: rivet::core::BodyKind::GraphQl,
        text: "{ me { id } }".into(),
        variables: "{\"a\": 1}".into(),
        ..Body::default()
    };
    let (_, json) = send(&gql, None);
    assert_eq!(json["body"], r#"{"query":"{ me { id } }","variables":{"a":1}}"#);
    assert_eq!(json["headers"]["content-type"], "application/json");

    let spec = RequestSpec::new("opt", Method::Options, format!("{base}/echo"));
    let (_, json) = send(&spec, None);
    assert_eq!(json["method"], "OPTIONS");
}

#[test]
fn not_found_is_ok() {
    let base = server();
    let spec = RequestSpec::new("404", Method::Get, format!("{base}/missing"));
    let (resp, json) = send(&spec, None);
    assert_eq!(resp.status, 404);
    assert_eq!(resp.status_text, "Not Found");
    assert_eq!(json["error"], "not found");
}

#[test]
fn head_has_no_body() {
    let base = server();
    let spec = RequestSpec::new("head", Method::Head, format!("{base}/echo"));
    let (resp, _) = send(&spec, None);
    assert_eq!(resp.status, 200);
    assert!(resp.body.is_empty());
    assert!(resp.header("content-length").is_some());
}

#[test]
fn query_params_merge_and_encode() {
    let base = server();
    let mut spec = RequestSpec::new("q", Method::Get, format!("{base}/echo?a=1"));
    spec.params = vec![
        KeyValue::new("q", "hello world&more"),
        KeyValue::new("id", "{{id}}"),
        KeyValue::new("off", "x").enabled(false),
    ];
    let (_, json) = send(&spec, Some(&env(&base)));
    assert_eq!(json["target"], "/echo?a=1&q=hello%20world%26more&id=42");
}

#[test]
fn vars_and_missing() {
    let base = server();
    let mut spec = RequestSpec::new("v", Method::Get, "{{baseUrl}}/echo/{{id}}");
    spec.headers = vec![KeyValue::new("X-Who", "{{user}}")];
    let req = resolve(&spec, Some(&env(&base))).unwrap();
    assert_eq!(req.url, format!("{base}/echo/42"));
    assert_eq!(req.missing_vars, vec!["user"]);
    let resp = execute(&req, &Settings::default(), &CancelToken::default()).unwrap();
    let json: Value = serde_json::from_slice(&resp.body).unwrap();
    // Undefined variables go out as empty strings, never as literal braces.
    assert_eq!(json["headers"]["x-who"], "");

    // UTF-8 header values are sent (obs-text) instead of failing in hyper.
    let mut spec = RequestSpec::new("v", Method::Get, "{{baseUrl}}/echo");
    spec.headers = vec![KeyValue::new("X-Name", "Иван")];
    let req = resolve(&spec, Some(&env(&base))).unwrap();
    let resp = execute(&req, &Settings::default(), &CancelToken::default()).unwrap();
    let json: Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(json["headers"]["x-name"], "Иван");

    // Unresolved host is an invalid URL.
    let spec = RequestSpec::new("v", Method::Get, "{{baseUrl}}/x");
    assert!(matches!(
        resolve(&spec, None),
        Err(SendError::InvalidUrl(_))
    ));
}

#[test]
fn auth_headers() {
    let base = server();
    let e = env(&base);
    let mut spec = RequestSpec::new("a", Method::Get, format!("{base}/echo"));

    spec.auth = Auth::Bearer {
        token: "{{token}}".into(),
    };
    assert_eq!(
        send(&spec, Some(&e)).1["headers"]["authorization"],
        "Bearer envtok"
    );

    spec.auth = Auth::Inherit;
    assert_eq!(
        send(&spec, Some(&e)).1["headers"]["authorization"],
        "Bearer envtok"
    );

    spec.auth = Auth::Basic {
        username: "Aladdin".into(),
        password: "open sesame".into(),
    };
    assert_eq!(
        send(&spec, Some(&e)).1["headers"]["authorization"],
        "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
    );

    spec.auth = Auth::ApiKey {
        key: "X-Api-Key".into(),
        value: "k1".into(),
        in_query: false,
    };
    assert_eq!(send(&spec, Some(&e)).1["headers"]["x-api-key"], "k1");

    spec.auth = Auth::ApiKey {
        key: "api_key".into(),
        value: "k 2".into(),
        in_query: true,
    };
    assert_eq!(send(&spec, Some(&e)).1["target"], "/echo?api_key=k%202");
}

#[test]
fn connection_refused() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let spec = RequestSpec::new("x", Method::Get, format!("127.0.0.1:{port}/"));
    let req = resolve(&spec, None).unwrap();
    match execute(&req, &Settings::default(), &CancelToken::default()) {
        Err(SendError::Network { code, host, .. }) => {
            assert_eq!(code, "ECONNREFUSED");
            assert_eq!(host, format!("127.0.0.1:{port}"));
        }
        other => panic!("expected ECONNREFUSED, got {other:?}"),
    }
}

#[test]
fn host_not_found() {
    let spec = RequestSpec::new("x", Method::Get, "http://no-such-host.invalid/");
    let req = resolve(&spec, None).unwrap();
    match execute(&req, &Settings::default(), &CancelToken::default()) {
        Err(SendError::Network { code, .. }) => assert_eq!(code, "ENOTFOUND"),
        other => panic!("expected ENOTFOUND, got {other:?}"),
    }
}

#[test]
fn timeout() {
    let base = server();
    let spec = RequestSpec::new("slow", Method::Get, format!("{base}/sleep"));
    let req = resolve(&spec, None).unwrap();
    let settings = Settings {
        timeout_ms: 300,
        ..Settings::default()
    };
    let started = Instant::now();
    let res = execute(&req, &settings, &CancelToken::default());
    assert_eq!(res, Err(SendError::Timeout { after_ms: 300 }));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn cancel() {
    let base = server();
    let spec = RequestSpec::new("slow", Method::Get, format!("{base}/sleep"));
    let req = resolve(&spec, None).unwrap();
    let token = CancelToken::default();
    let t = token.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        t.cancel();
    });
    let started = Instant::now();
    assert_eq!(
        execute(&req, &Settings::default(), &token),
        Err(SendError::Cancelled)
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn curl_round_trip() {
    let base = server();
    let mut spec = RequestSpec::new("c", Method::Patch, format!("{base}/echo?x=1"));
    spec.params = vec![KeyValue::new("q", "a b")];
    spec.headers = vec![KeyValue::new("X-Quote", "it's")];
    spec.body = Body::json(r#"{"name": "O'Brien"}"#);
    spec.auth = Auth::Bearer {
        token: "t0k".into(),
    };
    let original = resolve(&spec, None).unwrap();

    let cmd = to_curl(&original);
    let parsed = parse_curl(&cmd).expect("parse back");
    assert_eq!(parsed.method, Method::Patch);
    assert_eq!(
        parsed.auth,
        Auth::Bearer {
            token: "t0k".into()
        }
    );
    let again = resolve(&parsed, None).unwrap();
    assert_eq!(again.url, original.url);
    assert_eq!(again.body, original.body);
    let mut a = again.headers.clone();
    let mut b = original.headers.clone();
    a.sort();
    b.sort();
    assert_eq!(a, b);

    let resp = execute(&again, &Settings::default(), &CancelToken::default()).unwrap();
    let json: Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(json["method"], "PATCH");
    assert_eq!(json["body"], r#"{"name": "O'Brien"}"#);
    assert_eq!(json["headers"]["x-quote"], "it's");
}

/// Needs internet: `cargo test --test core_http -- --ignored --nocapture`.
#[test]
#[ignore]
fn https_real_hosts() {
    let spec = RequestSpec::new("x", Method::Get, "https://example.com/");
    let req = resolve(&spec, None).unwrap();
    let resp = execute(&req, &Settings::default(), &CancelToken::default()).unwrap();
    println!(
        "example.com: {} {:?} {:?}",
        resp.status, resp.remote_addr, resp.timings
    );
    assert_eq!(resp.status, 200);
    assert!(resp.timings.tls > Duration::ZERO);

    let spec = RequestSpec::new("x", Method::Get, "https://self-signed.badssl.com/");
    let req = resolve(&spec, None).unwrap();
    let err = execute(&req, &Settings::default(), &CancelToken::default()).unwrap_err();
    println!("self-signed verify=on: {err:?}");
    assert!(matches!(err, SendError::Network { ref code, .. } if code == "TLS"));
    let lax = Settings {
        ssl_verify: false,
        ..Settings::default()
    };
    let resp = execute(&req, &lax, &CancelToken::default()).unwrap();
    println!("self-signed verify=off: {}", resp.status);
}

// ---------------------------------------------------------------------------
// HTTP/2, TLS, proxies, decoding.

mod h2 {
    use std::convert::Infallible;
    use std::sync::Arc;

    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::body::Incoming;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use serde_json::Value;

    /// Runs `serve` on a fresh current-thread runtime in a background thread.
    fn background<F>(listener: std::net::TcpListener, serve: F)
    where
        F: Fn(tokio::net::TcpStream) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>>
            + Send
            + 'static,
    {
        listener.set_nonblocking(true).unwrap();
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                    while let Ok((tcp, _)) = listener.accept().await {
                        tokio::spawn(serve(tcp));
                    }
                });
        });
    }

    async fn echo(req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
        let method = req.method().to_string();
        let version = format!("{:?}", req.version());
        let target = req
            .uri()
            .path_and_query()
            .map(|p| p.as_str().to_string())
            .unwrap_or_default();
        let headers: serde_json::Map<String, Value> = req
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_string(),
                    Value::String(v.to_str().unwrap_or("").to_string()),
                )
            })
            .collect();
        let body = req
            .into_body()
            .collect()
            .await
            .map(|b| b.to_bytes())
            .unwrap_or_default();
        let json = serde_json::json!({
            "method": method,
            "version": version,
            "target": target,
            "headers": headers,
            "body": String::from_utf8_lossy(&body),
        });
        Ok(Response::builder()
            .header("content-type", "application/json")
            .header("x-multi", "a")
            .header("x-multi", "b")
            .body(Full::new(Bytes::from(json.to_string())))
            .unwrap())
    }

    async fn serve_io<I>(io: I, h2: bool)
    where
        I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
    {
        if h2 {
            let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                .serve_connection(io, service_fn(echo))
                .await;
        } else {
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, service_fn(echo))
                .await;
        }
    }

    /// Plain-TCP HTTP/2 server (h2c, prior knowledge only). Returns the base URL.
    pub fn h2c_server() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        background(listener, |tcp| Box::pin(serve_io(TokioIo::new(tcp), true)));
        format!("http://{addr}")
    }

    /// TLS server with a fresh self-signed certificate offering ALPN
    /// `h2` and `http/1.1`. Returns the base URL.
    pub fn tls_server() -> String {
        let cert =
            rcgen::generate_simple_self_signed(vec!["127.0.0.1".into(), "localhost".into()])
                .unwrap();
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(
            rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()),
        );
        let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.cert.der().clone()], key)
        .unwrap();
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        background(listener, move |tcp| {
            let acceptor = acceptor.clone();
            Box::pin(async move {
                let Ok(tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let h2 = tls.get_ref().1.alpn_protocol() == Some(b"h2");
                serve_io(TokioIo::new(tls), h2).await;
            })
        });
        format!("https://{addr}")
    }

    /// Minimal HTTP CONNECT proxy. Returns `http://addr` and records every
    /// CONNECT target it was asked for.
    pub fn connect_proxy() -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let log = seen.clone();
        background(listener, move |mut tcp| {
            let log = log.clone();
            Box::pin(async move {
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    let Ok(b) = tcp.read_u8().await else { return };
                    head.push(b);
                }
                let text = String::from_utf8_lossy(&head).to_string();
                let target = text.split_whitespace().nth(1).unwrap_or("").to_string();
                log.lock().unwrap().push(text.lines().next().unwrap_or("").to_string());
                let Ok(mut upstream) = tokio::net::TcpStream::connect(&target).await else {
                    let _ = tcp.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
                    return;
                };
                let _ = tcp.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").await;
                let _ = tokio::io::copy_bidirectional(&mut tcp, &mut upstream).await;
            })
        });
        (format!("http://{addr}"), seen)
    }
}

fn settings_with(f: impl FnOnce(&mut Settings)) -> Settings {
    let mut s = Settings::default();
    f(&mut s);
    s
}

#[test]
fn h2c_prior_knowledge() {
    let base = h2::h2c_server();
    let mut spec = RequestSpec::new("h2c", Method::Post, format!("{base}/echo?x=1"));
    spec.headers = vec![KeyValue::new("X-Trace", "abc")];
    spec.body = Body::json(r#"{"a":1}"#);
    let req = resolve(&spec, None).unwrap();
    let settings = settings_with(|s| s.http_version = HttpVersionPref::Http2);
    let resp = execute(&req, &settings, &CancelToken::default()).unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.status_text, "OK");
    assert_eq!(resp.http_version, "HTTP/2");
    assert!(resp.remote_addr.as_deref().unwrap().starts_with("127.0.0.1:"));
    let json: Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(json["version"], "HTTP/2.0");
    assert_eq!(json["method"], "POST");
    assert_eq!(json["target"], "/echo?x=1");
    assert_eq!(json["body"], r#"{"a":1}"#);
    assert_eq!(json["headers"]["x-trace"], "abc");
    assert_eq!(json["headers"]["content-type"], "application/json");
    let multi: Vec<&str> = resp
        .headers
        .iter()
        .filter(|(k, _)| k == "x-multi")
        .map(|(_, v)| v.as_str())
        .collect();
    assert_eq!(multi, vec!["a", "b"]);

    // Auto never tries h2c on plain http: the h2-only server rejects HTTP/1.1.
    assert!(execute(&req, &Settings::default(), &CancelToken::default()).is_err());
}

#[test]
fn tls_alpn_negotiates_http2() {
    let base = h2::tls_server();
    let url = format!("{base}/echo");
    let lax = settings_with(|s| s.ssl_verify = false);
    let resp = get(url.clone(), &lax);
    assert_eq!(resp.status, 200);
    assert_eq!(resp.http_version, "HTTP/2");
    let json: Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(json["version"], "HTTP/2.0");
    assert_eq!(json["target"], "/echo");

    let t = &resp.timings;
    assert!(t.tls > Duration::ZERO);
    assert!(t.connect > Duration::ZERO);
    assert!(t.dns + t.connect + t.tls + t.ttfb + t.download <= t.total);

    // Forced HTTP/1.1 over the same server.
    let h1 = settings_with(|s| {
        s.ssl_verify = false;
        s.http_version = HttpVersionPref::Http1;
    });
    let resp = get(url.clone(), &h1);
    assert_eq!(resp.http_version, "HTTP/1.1");
    let json: Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(json["version"], "HTTP/1.1");

    // Forced HTTP/2 negotiates h2 too.
    let h2 = settings_with(|s| {
        s.ssl_verify = false;
        s.http_version = HttpVersionPref::Http2;
    });
    assert_eq!(get(url.clone(), &h2).http_version, "HTTP/2");

    // Self-signed certificate is rejected when verification is on.
    let req = resolve(&RequestSpec::new("r", Method::Get, url), None).unwrap();
    match execute(&req, &Settings::default(), &CancelToken::default()) {
        Err(SendError::Network { code, .. }) => assert_eq!(code, "TLS"),
        other => panic!("expected TLS error, got {other:?}"),
    }
}

#[test]
fn https_through_connect_proxy() {
    let base = h2::tls_server();
    let (proxy, seen) = h2::connect_proxy();
    let settings = settings_with(|s| {
        s.ssl_verify = false;
        s.proxy = Some(proxy);
    });
    let resp = get(format!("{base}/echo"), &settings);
    assert_eq!(resp.status, 200);
    assert_eq!(resp.http_version, "HTTP/2");
    assert_eq!(resp.remote_addr, None);
    let authority = base.trim_start_matches("https://");
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        [format!("CONNECT {authority} HTTP/1.1")]
    );
}

#[test]
fn plain_http_proxy_uses_absolute_form() {
    // The echo server stands in for the proxy: it echoes the request target.
    let proxy = server();
    let settings = settings_with(|s| s.proxy = Some(proxy.clone()));
    let resp = get("http://example.invalid:81/echo?q=1".into(), &settings);
    let json: Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(json["target"], "http://example.invalid:81/echo?q=1");
    assert_eq!(json["headers"]["host"], "example.invalid:81");

    let bad = settings_with(|s| s.proxy = Some("socks5://127.0.0.1:1".into()));
    let req = resolve(&RequestSpec::new("r", Method::Get, proxy), None).unwrap();
    assert!(matches!(
        execute(&req, &bad, &CancelToken::default()),
        Err(SendError::Network { code, .. }) if code == "EIO"
    ));
}

#[test]
fn gzip_body_is_decoded() {
    let base = server();
    let resp = get(format!("{base}/gzip"), &Settings::default());
    assert_eq!(resp.body, br#"{"zipped":true}"#);
    assert!(resp.header("content-encoding").is_none());
    assert!(resp.header("content-length").is_none());
    assert_eq!(resp.header("x-accept-encoding"), Some("gzip, deflate, br"));
}

#[test]
fn set_cookie_on_redirect_applies_to_next_hop() {
    let base = server();
    let resp = get(format!("{base}/cookie/redirect"), &Settings::default());
    assert_eq!(resp.hops.len(), 1);
    let json: Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(json["headers"]["cookie"], "hop=1");
    // Unscoped requests use a throwaway jar.
    let resp = get(format!("{base}/echo"), &Settings::default());
    let json: Value = serde_json::from_slice(&resp.body).unwrap();
    assert!(json["headers"].get("cookie").is_none());
}

#[test]
fn settings_http_version_serde() {
    let s: Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(s.http_version, HttpVersionPref::Auto);
    let s: Settings = serde_json::from_str(r#"{"http_version":"http2"}"#).unwrap();
    assert_eq!(s.http_version, HttpVersionPref::Http2);
    let out = serde_json::to_value(settings_with(|s| s.http_version = HttpVersionPref::Http1))
        .unwrap();
    assert_eq!(out["http_version"], "http1");
}
