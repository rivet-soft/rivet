//! Import of HAR 1.2 files (DevTools "Save all as HAR", proxies): every entry
//! becomes a request — query string as params, body by MIME type, a folder
//! per host — and its recorded response becomes an example.

use serde_json::Value;

use super::types::{Body, BodyKind, Collection, Example, KeyValue, Method, RequestSpec, new_id};
use super::vars::decode_component;

/// A HAR document: `{"log": {"entries": [...]}}`.
pub fn is_har(doc: &Value) -> bool {
    doc.pointer("/log/entries").is_some_and(Value::is_array)
}

/// Headers the client sets itself (or HTTP/2 pseudo-headers): not copied.
fn skipped_header(name: &str) -> bool {
    name.starts_with(':')
        || matches!(
            name.to_ascii_lowercase().as_str(),
            "host" | "content-length" | "connection" | "transfer-encoding" | "accept-encoding"
        )
}

fn str_field<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn pairs(v: Option<&Value>) -> Vec<(String, String)> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|p| (str_field(p, "name").to_string(), str_field(p, "value").to_string()))
        .collect()
}

/// `https://h/a/b` → `/a/b` (for request names).
fn path_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.find('/').map_or("/", |i| &rest[i..])
}

fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}

fn body_of(post: &Value, warnings: &mut Vec<String>, name: &str) -> Body {
    let mime = str_field(post, "mimeType").to_ascii_lowercase();
    let text = str_field(post, "text").to_string();
    let params = post.get("params").and_then(Value::as_array);
    if mime.contains("multipart/form-data") {
        let form = params
            .into_iter()
            .flatten()
            .map(|p| {
                let file_name = str_field(p, "fileName");
                let mut kv = KeyValue::new(str_field(p, "name"), if file_name.is_empty() { str_field(p, "value") } else { file_name });
                kv.file = !file_name.is_empty();
                kv
            })
            .collect::<Vec<_>>();
        if form.iter().any(|f| f.file) {
            warnings.push(format!("«{name}»: файлы multipart не входят в HAR — укажите их заново"));
        }
        return Body { kind: BodyKind::Multipart, form, ..Body::default() };
    }
    if mime.contains("x-www-form-urlencoded") {
        let form = match params.filter(|p| !p.is_empty()) {
            Some(p) => p.iter().map(|p| KeyValue::new(decode_component(str_field(p, "name"), true), decode_component(str_field(p, "value"), true))).collect(),
            None => text
                .split('&')
                .filter(|s| !s.is_empty())
                .map(|pair| {
                    let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                    KeyValue::new(decode_component(k, true), decode_component(v, true))
                })
                .collect(),
        };
        return Body { kind: BodyKind::Form, form, ..Body::default() };
    }
    if text.is_empty() {
        return Body::default();
    }
    let json = mime.contains("json") || serde_json::from_str::<Value>(&text).is_ok_and(|v| v.is_object() || v.is_array());
    Body { kind: if json { BodyKind::Json } else { BodyKind::Text }, text, ..Body::default() }
}

fn example_of(response: &Value, url: &str, warnings: &mut Vec<String>, name: &str) -> Option<Example> {
    let status = response.get("status").and_then(Value::as_u64)? as u16;
    if status == 0 {
        return None; // blocked / failed in the browser
    }
    let content = response.get("content");
    let encoding = content.map(|c| str_field(c, "encoding")).unwrap_or("");
    let mut body = content.map(|c| str_field(c, "text").to_string()).unwrap_or_default();
    if encoding.eq_ignore_ascii_case("base64") && !body.is_empty() {
        warnings.push(format!("«{name}»: двоичный ответ не сохранён в пример"));
        body.clear();
    }
    if body.len() > Example::MAX_BODY {
        body.clear();
    }
    let status_text = str_field(response, "statusText").to_string();
    Some(Example {
        id: new_id(),
        name: Example::default_name(status, &status_text),
        status,
        status_text,
        headers: pairs(response.get("headers")).into_iter().filter(|(k, _)| !k.starts_with(':')).map(|(k, v)| KeyValue::new(k, v)).collect(),
        body,
        url: Some(url.to_string()),
        saved_at: 0,
    })
}

/// Collection `name` from a HAR document, plus notes for the user.
pub fn import(doc: &Value, name: &str) -> Result<(Collection, Vec<String>), String> {
    if !is_har(doc) {
        return Err("Это не HAR-файл (нет log.entries)".into());
    }
    let mut collection = Collection::new(name);
    let mut warnings = Vec::new();
    let mut skipped = 0usize;
    for entry in doc.pointer("/log/entries").and_then(Value::as_array).into_iter().flatten() {
        let Some(req) = entry.get("request") else {
            skipped += 1;
            continue;
        };
        let url = str_field(req, "url");
        let Some(method) = Method::parse(str_field(req, "method")) else {
            skipped += 1;
            continue;
        };
        if url.is_empty() || url.starts_with("data:") || url.starts_with("blob:") || url.starts_with("chrome-extension:") {
            skipped += 1;
            continue;
        }
        let base = url.split(['?', '#']).next().unwrap_or(url);
        let title = format!("{} {}", method.full_label(), path_of(base));
        let mut request = RequestSpec::new(&title, method, base);
        request.collection_id = Some(collection.id.clone());
        let host = host_of(url).to_string();
        if !collection.folders.contains(&host) {
            collection.folders.push(host.clone());
        }
        request.folder = Some(host);
        let query = pairs(req.get("queryString"));
        request.params = if query.is_empty() {
            url.split_once('?')
                .map(|(_, q)| q.split('#').next().unwrap_or(""))
                .into_iter()
                .flat_map(|q| q.split('&'))
                .filter(|s| !s.is_empty())
                .map(|pair| {
                    let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                    KeyValue::new(decode_component(k, true), decode_component(v, true))
                })
                .collect()
        } else {
            query.into_iter().map(|(k, v)| KeyValue::new(k, v)).collect()
        };
        request.headers = pairs(req.get("headers"))
            .into_iter()
            .filter(|(k, _)| !skipped_header(k))
            .map(|(k, v)| KeyValue::new(k, v))
            .collect();
        if let Some(post) = req.get("postData") {
            request.body = body_of(post, &mut warnings, &title);
            if request.body.kind == BodyKind::Multipart {
                // The library builds the boundary; the recorded one is stale.
                request.headers.retain(|h| !h.key.eq_ignore_ascii_case("content-type"));
            }
        }
        if let Some(example) = entry.get("response").and_then(|r| example_of(r, url, &mut warnings, &title)) {
            request.examples.push(example);
        }
        collection.requests.push(request);
    }
    if skipped > 0 {
        warnings.push(format!("пропущено записей: {skipped} (data:/blob: URL, неизвестный метод)"));
    }
    Ok((collection, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn har() -> Value {
        json!({"log": {"version": "1.2", "entries": [
            {"request": {"method": "GET", "url": "https://api.x.com/v1/users?limit=5&q=a%20b",
                "headers": [{"name": ":authority", "value": "api.x.com"}, {"name": "Accept", "value": "application/json"}, {"name": "Accept-Encoding", "value": "gzip"}, {"name": "Authorization", "value": "Bearer t"}],
                "queryString": [{"name": "limit", "value": "5"}, {"name": "q", "value": "a b"}]},
             "response": {"status": 200, "statusText": "OK", "headers": [{"name": "content-type", "value": "application/json"}],
                "content": {"mimeType": "application/json", "text": "[{\"id\":1}]"}}},
            {"request": {"method": "POST", "url": "https://api.x.com/v1/login", "headers": [{"name": "Content-Type", "value": "application/x-www-form-urlencoded"}],
                "postData": {"mimeType": "application/x-www-form-urlencoded", "text": "user=ann&pass=a%26b"}},
             "response": {"status": 302, "statusText": "Found", "headers": [], "content": {"text": ""}}},
            {"request": {"method": "PUT", "url": "https://cdn.x.com/up", "headers": [{"name": "Content-Type", "value": "multipart/form-data; boundary=abc"}],
                "postData": {"mimeType": "multipart/form-data; boundary=abc", "params": [{"name": "note", "value": "hi"}, {"name": "file", "fileName": "a.png"}]}},
             "response": {"status": 201, "statusText": "", "content": {"text": "AAEC", "encoding": "base64"}}},
            {"request": {"method": "PATCH", "url": "https://api.x.com/v1/users/1", "headers": [],
                "postData": {"mimeType": "text/plain", "text": "{\"name\":\"B\"}"}},
             "response": {"status": 0}},
            {"request": {"method": "GET", "url": "data:image/png;base64,AAA"}, "response": {"status": 200}}
        ]}})
    }

    #[test]
    fn entries_become_requests_with_examples() {
        let (c, warnings) = import(&har(), "Trace").unwrap();
        assert_eq!(c.name, "Trace");
        assert_eq!(c.folders, ["api.x.com", "cdn.x.com"]);
        assert_eq!(c.requests.len(), 4);
        let list = &c.requests[0];
        assert_eq!(list.name, "GET /v1/users");
        assert_eq!(list.url, "https://api.x.com/v1/users");
        assert_eq!(list.params, vec![KeyValue::new("limit", "5"), KeyValue::new("q", "a b")]);
        assert_eq!(list.headers, vec![KeyValue::new("Accept", "application/json"), KeyValue::new("Authorization", "Bearer t")]);
        assert_eq!(list.examples[0].body, "[{\"id\":1}]");
        assert_eq!(list.examples[0].name, "200 OK");
        assert!(c.requests.iter().all(|r| r.collection_id.as_deref() == Some(c.id.as_str())));

        let login = &c.requests[1];
        assert_eq!(login.body.kind, BodyKind::Form);
        assert_eq!(login.body.form, vec![KeyValue::new("user", "ann"), KeyValue::new("pass", "a&b")]);
        assert_eq!(login.examples[0].status, 302);

        let up = &c.requests[2];
        assert_eq!(up.folder.as_deref(), Some("cdn.x.com"));
        assert_eq!(up.body.kind, BodyKind::Multipart);
        assert!(up.body.form[1].file);
        assert!(up.headers.is_empty(), "stale multipart boundary dropped");
        assert!(up.examples[0].body.is_empty(), "base64 body not kept");

        let patch = &c.requests[3];
        assert_eq!(patch.body.kind, BodyKind::Json, "JSON text sniffed despite text/plain");
        assert!(patch.examples.is_empty(), "status 0 = no response");

        assert!(warnings.iter().any(|w| w.contains("multipart")));
        assert!(warnings.iter().any(|w| w.contains("двоичный")));
        assert!(warnings.iter().any(|w| w.contains("пропущено записей: 1")));
        assert!(import(&json!({"info": {}}), "x").is_err());
    }
}
