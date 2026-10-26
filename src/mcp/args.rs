//! Tool argument access and conversion of request JSON into [`RequestSpec`]
//! fields (accepts the shapes `get_request` returns plus friendlier shorthands).

use serde_json::{Map, Value};

use crate::core::project_ops::FOLDER_SEP;
use crate::core::{Auth, Body, BodyKind, KeyValue, Method, RequestSpec};

pub struct Args<'a>(pub &'a Map<String, Value>);

impl<'a> Args<'a> {
    pub fn new(value: &'a Value) -> Self {
        static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
        Args(value.as_object().unwrap_or_else(|| EMPTY.get_or_init(Map::new)))
    }

    pub fn has(&self, key: &str) -> bool {
        self.0.get(key).is_some_and(|v| !v.is_null())
    }

    pub fn get(&self, key: &str) -> Option<&'a Value> {
        self.0.get(key).filter(|v| !v.is_null())
    }

    /// Non-empty string argument.
    pub fn str(&self, key: &str) -> Result<Option<&'a str>, String> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.as_str())),
            Some(_) => Err(format!("'{key}' must be a string")),
        }
    }

    pub fn req_str(&self, key: &str) -> Result<&'a str, String> {
        self.str(key)?.ok_or_else(|| format!("'{key}' is required"))
    }

    pub fn bool(&self, key: &str) -> Result<Option<bool>, String> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(format!("'{key}' must be true or false")),
        }
    }

    pub fn flag(&self, key: &str) -> Result<bool, String> {
        Ok(self.bool(key)?.unwrap_or(false))
    }

    pub fn u64(&self, key: &str) -> Result<Option<u64>, String> {
        match self.get(key) {
            None => Ok(None),
            Some(v) => v
                .as_u64()
                .or_else(|| v.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64))
                .map(Some)
                .ok_or_else(|| format!("'{key}' must be a non-negative number")),
        }
    }
}

fn scalar_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// `[{key, value, enabled?, description?, file?}]` or `{"key": "value"}`.
pub fn key_values(value: &Value, field: &str) -> Result<Vec<KeyValue>, String> {
    match value {
        Value::Null => Ok(Vec::new()),
        Value::Object(map) => Ok(map.iter().map(|(k, v)| KeyValue::new(k.clone(), scalar_text(v))).collect()),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                let obj = item
                    .as_object()
                    .ok_or_else(|| format!("'{field}' items must be objects like {{\"key\":\"…\",\"value\":\"…\"}}"))?;
                let key = obj
                    .get("key")
                    .or_else(|| obj.get("name"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("'{field}' item without a 'key'"))?;
                let mut kv = KeyValue::new(key, obj.get("value").map(scalar_text).unwrap_or_default());
                kv.enabled = obj.get("enabled").and_then(Value::as_bool).unwrap_or(true);
                kv.description = obj.get("description").and_then(Value::as_str).unwrap_or("").to_string();
                kv.file = obj.get("file").and_then(Value::as_bool).unwrap_or(false);
                Ok(kv)
            })
            .collect(),
        _ => Err(format!("'{field}' must be an array of {{key, value}} objects or a key→value object")),
    }
}

fn parse_method(text: &str) -> Result<Method, String> {
    let t = text.trim();
    Method::parse(t)
        .or_else(|| Method::ALL.into_iter().find(|m| m.label().eq_ignore_ascii_case(t)))
        .ok_or_else(|| format!("Unknown method '{t}'; use GET, POST, PUT, PATCH, DELETE, HEAD or OPTIONS"))
}

/// Body from a string (JSON when it parses, else text), a JSON object/array
/// (sent as JSON), or `{kind, text, form, file_path}`.
pub fn body(value: &Value) -> Result<Body, String> {
    match value {
        Value::Null => Ok(Body::default()),
        Value::String(s) if s.is_empty() => Ok(Body::default()),
        Value::String(s) => {
            let is_json = serde_json::from_str::<Value>(s).is_ok_and(|v| v.is_object() || v.is_array());
            Ok(Body { kind: if is_json { BodyKind::Json } else { BodyKind::Text }, text: s.clone(), ..Default::default() })
        }
        Value::Object(obj) if obj.contains_key("kind") => {
            let kind: BodyKind = serde_json::from_value(obj["kind"].clone())
                .map_err(|_| "body.kind must be none, json, text, form, multipart, binary or graphql".to_string())?;
            let text = match obj.get("text").or_else(|| obj.get("json")).or_else(|| obj.get("query")) {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(s)) => s.clone(),
                Some(other) => serde_json::to_string_pretty(other).unwrap_or_default(),
            };
            let form = obj.get("form").map(|f| key_values(f, "body.form")).transpose()?.unwrap_or_default();
            let file_path = obj.get("file_path").and_then(Value::as_str).map(std::path::PathBuf::from);
            // GraphQL variables: a JSON object or JSON text.
            let variables = match obj.get("variables") {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(s)) => s.clone(),
                Some(other) => serde_json::to_string_pretty(other).unwrap_or_default(),
            };
            Ok(Body { kind, text, form, file_path, variables })
        }
        other => Ok(Body::json(serde_json::to_string_pretty(other).unwrap_or_default())),
    }
}

/// `{type: none|inherit|bearer|basic|api_key, …}` or a bare type string.
pub fn auth(value: &Value) -> Result<Auth, String> {
    let (kind, obj) = match value {
        Value::Null => return Ok(Auth::None),
        Value::String(s) => (s.as_str(), None),
        Value::Object(obj) => (obj.get("type").and_then(Value::as_str).unwrap_or("none"), Some(obj)),
        _ => return Err("auth must be an object like {\"type\":\"bearer\",\"token\":\"…\"}".into()),
    };
    let field = |name: &str| obj.and_then(|o| o.get(name)).map(scalar_text).unwrap_or_default();
    match kind.to_ascii_lowercase().replace('-', "_").as_str() {
        "none" | "" => Ok(Auth::None),
        "inherit" => Ok(Auth::Inherit),
        "bearer" => Ok(Auth::Bearer { token: field("token") }),
        "basic" => Ok(Auth::Basic { username: field("username"), password: field("password") }),
        "api_key" | "apikey" => Ok(Auth::ApiKey {
            key: field("key"),
            value: field("value"),
            in_query: obj.and_then(|o| o.get("in_query")).and_then(Value::as_bool).unwrap_or(false),
        }),
        other => Err(format!("Unknown auth type '{other}'; use none, inherit, bearer, basic or api_key")),
    }
}

/// `a/b`, `a / b` → `a / b`; empty → `None`.
pub fn normalize_folder(folder: &str) -> Option<String> {
    let parts: Vec<&str> = folder.split('/').map(str::trim).filter(|p| !p.is_empty()).collect();
    (!parts.is_empty()).then(|| parts.join(FOLDER_SEP))
}

/// Applies the request fields present in `args` to `request`.
pub fn apply_request_fields(request: &mut RequestSpec, args: &Args) -> Result<(), String> {
    if let Some(name) = args.str("name")? {
        request.name = name.to_string();
    }
    if let Some(method) = args.str("method")? {
        request.method = parse_method(method)?;
    }
    if let Some(url) = args.get("url") {
        request.url = url.as_str().ok_or("'url' must be a string")?.trim().to_string();
    }
    if let Some(v) = args.0.get("params") {
        request.params = key_values(v, "params")?;
    }
    if let Some(v) = args.0.get("headers") {
        request.headers = key_values(v, "headers")?;
    }
    if let Some(v) = args.0.get("body") {
        request.body = body(v)?;
    }
    if let Some(v) = args.0.get("auth") {
        request.auth = auth(v)?;
    }
    if let Some(scripts) = args.get("scripts") {
        let obj = scripts.as_object().ok_or("'scripts' must be an object {pre_request, post_response}")?;
        if let Some(pre) = obj.get("pre_request") {
            request.scripts.pre_request = scalar_text(pre);
        }
        if let Some(post) = obj.get("post_response") {
            request.scripts.post_response = scalar_text(post);
        }
    }
    if args.0.contains_key("follow_redirects") {
        request.follow_redirects = args.bool("follow_redirects")?;
    }
    if args.0.contains_key("max_redirects") {
        request.max_redirects = args.u64("max_redirects")?.map(|n| n.min(u32::MAX as u64) as u32);
    }
    if let Some(description) = args.get("description") {
        request.description = scalar_text(description);
    }
    if let Some(v) = args.0.get("path_variables") {
        // Values for `:name` segments; rows follow the URL's names.
        let given = key_values(v, "path_variables")?;
        request.path_vars = crate::core::vars::sync_path_vars(&request.url, &given);
    } else if args.0.contains_key("url") {
        request.path_vars = crate::core::vars::sync_path_vars(&request.url, &request.path_vars);
    }
    if args.0.contains_key("request_timeout_ms") {
        request.timeout_ms = args.u64("request_timeout_ms")?.map(|n| n.clamp(1, 3_600_000));
    }
    if args.0.contains_key("ssl_verify") {
        request.ssl_verify = args.bool("ssl_verify")?;
    }
    if let Some(v) = args.0.get("disabled_auto_headers") {
        let names = v.as_array().ok_or("disabled_auto_headers: expected an array of header names")?;
        request.disabled_auto_headers.clear();
        for name in names {
            let name = name.as_str().ok_or("disabled_auto_headers: expected header names")?;
            crate::core::auto_headers::set_enabled(request, name.trim(), false);
        }
    }
    Ok(())
}

/// Default name for a request without one: the last URL path segment.
pub fn name_from_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let path = path.split_once("://").map_or(path, |(_, rest)| rest.split_once('/').map_or("", |(_, p)| p));
    path.rsplit('/')
        .find(|s| !s.is_empty() && !s.starts_with("{{"))
        .map(String::from)
        .unwrap_or_else(|| "Untitled".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_request_shapes() {
        let args = json!({
            "method": "post", "url": " {{baseUrl}}/orders ",
            "headers": {"X-A": "1"},
            "params": [{"key": "limit", "value": 10, "enabled": false}],
            "body": {"item": 1},
            "auth": {"type": "api_key", "key": "X-Key", "value": "k"},
            "scripts": {"post_response": "pm.test('x', () => {})"}
        });
        let mut r = RequestSpec::untitled();
        apply_request_fields(&mut r, &Args::new(&args)).unwrap();
        assert_eq!(r.method, Method::Post);
        assert_eq!(r.url, "{{baseUrl}}/orders");
        assert_eq!(r.headers[0].key, "X-A");
        assert_eq!((r.params[0].value.as_str(), r.params[0].enabled), ("10", false));
        assert_eq!(r.body.kind, BodyKind::Json);
        assert!(matches!(r.auth, Auth::ApiKey { in_query: false, .. }));
        assert!(r.scripts.pre_request.is_empty() && !r.scripts.post_response.is_empty());

        let round = serde_json::to_value(&r).unwrap();
        let mut again = RequestSpec::untitled();
        apply_request_fields(&mut again, &Args::new(&round)).unwrap();
        assert_eq!((again.body.clone(), again.auth.clone(), again.params.clone()), (r.body, r.auth, r.params));

        let off = json!({"disabled_auto_headers": ["User-Agent", "authorization"]});
        apply_request_fields(&mut again, &Args::new(&off)).unwrap();
        assert_eq!(again.disabled_auto_headers, ["user-agent", "authorization"]);
        apply_request_fields(&mut again, &Args::new(&json!({"disabled_auto_headers": []}))).unwrap();
        assert!(again.disabled_auto_headers.is_empty());
        assert!(apply_request_fields(&mut again, &Args::new(&json!({"disabled_auto_headers": "accept"}))).is_err());

        assert_eq!(body(&json!("plain")).unwrap().kind, BodyKind::Text);
        assert!(apply_request_fields(&mut again, &Args::new(&json!({"method": "FETCH"}))).is_err());
        assert_eq!(normalize_folder("a/ b /"), Some("a / b".into()));
        assert_eq!(name_from_url("{{baseUrl}}/orders/{{id}}?x=1"), "orders");
        assert_eq!(name_from_url("https://example.com"), "Untitled");
    }
}
