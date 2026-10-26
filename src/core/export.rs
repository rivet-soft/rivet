//! Collection export in three formats:
//!
//! - **Rivet** (`*.rivet.json`): the collection as stored, lossless; imported
//!   back by [`super::import::import_file`].
//! - **Postman Collection v2.1**: folders, params/headers (disabled ones and
//!   descriptions kept), every body mode, auth, scripts, collection variables
//!   and redirect overrides (`protocolProfileBehavior`).
//! - **OpenAPI 3.0** (YAML/JSON): paths from the request URLs, parameters,
//!   request bodies with examples and inferred schemas, security schemes, tags
//!   from folders. What OpenAPI can't hold (scripts, disabled rows, redirect
//!   overrides, collection variables) goes into `x-rivet-*` extensions.
//!
//! Every exporter reports what it couldn't carry over in `warnings`.

use std::collections::{BTreeMap, HashSet};

use serde_json::{Map, Value, json};

use super::types::{Auth, BodyKind, Collection, KeyValue, RequestSpec};
use super::vars;

/// Export format offered in the UI and over MCP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Rivet,
    Postman,
    OpenApi,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 3] = [Self::Rivet, Self::Postman, Self::OpenApi];

    pub fn label(self) -> &'static str {
        match self {
            Self::Rivet => "Rivet (без потерь)",
            Self::Postman => "Postman Collection v2.1",
            Self::OpenApi => "OpenAPI 3.0 (YAML)",
        }
    }

    /// Identifier used by MCP and menu actions.
    pub fn id(self) -> &'static str {
        match self {
            Self::Rivet => "rivet",
            Self::Postman => "postman",
            Self::OpenApi => "openapi",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.id().eq_ignore_ascii_case(id))
    }

    /// Suggested file name for a collection called `name`.
    pub fn file_name(self, name: &str) -> String {
        let stem: String = name
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        let stem = stem.trim_matches('_');
        let stem = if stem.is_empty() { "collection" } else { stem };
        match self {
            Self::Rivet => format!("{stem}.rivet.json"),
            Self::Postman => format!("{stem}.postman_collection.json"),
            Self::OpenApi => format!("{stem}.openapi.yaml"),
        }
    }
}

/// Result of an export: file contents plus what didn't fit the format.
#[derive(Debug)]
pub struct Exported {
    pub text: String,
    pub warnings: Vec<String>,
}

/// Exports `collection` in `format`.
pub fn export(collection: &Collection, format: ExportFormat) -> Result<Exported, String> {
    match format {
        ExportFormat::Rivet => Ok(Exported { text: to_rivet(collection)?, warnings: Vec::new() }),
        ExportFormat::Postman => {
            let (doc, warnings) = to_postman(collection);
            let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
            Ok(Exported { text, warnings })
        }
        ExportFormat::OpenApi => {
            let (doc, warnings) = to_openapi(collection);
            let text = serde_yaml::to_string(&doc).map_err(|e| e.to_string())?;
            Ok(Exported { text, warnings })
        }
    }
}

// ---- Rivet -------------------------------------------------------------------

/// Marker of the native format: `{"rivet": "collection", "version": 1, ...}`.
pub const RIVET_MARKER: &str = "collection";

pub fn to_rivet(collection: &Collection) -> Result<String, String> {
    let doc = json!({
        "rivet": RIVET_MARKER,
        "version": 1,
        "collection": collection,
    });
    serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())
}

pub fn is_rivet(doc: &Value) -> bool {
    doc.get("rivet").and_then(Value::as_str) == Some(RIVET_MARKER) && doc.get("collection").is_some()
}

/// Reads a native export. Ids are regenerated so the copy never clashes with
/// the collection it was exported from.
pub fn from_rivet(doc: &Value) -> Result<Collection, String> {
    let mut collection: Collection = serde_json::from_value(doc["collection"].clone())
        .map_err(|e| format!("Некорректная коллекция Rivet: {e}"))?;
    collection.id = super::types::new_id();
    for request in &mut collection.requests {
        request.id = super::types::new_id();
        request.collection_id = Some(collection.id.clone());
    }
    Ok(collection)
}

// ---- Postman -----------------------------------------------------------------

const POSTMAN_SCHEMA: &str = "https://schema.getpostman.com/json/collection/v2.1.0/collection.json";

fn pm_kv(kv: &KeyValue) -> Value {
    let mut v = json!({"key": kv.key, "value": kv.value});
    if !kv.enabled {
        v["disabled"] = json!(true);
    }
    if !kv.description.is_empty() {
        v["description"] = json!(kv.description);
    }
    v
}

fn pm_auth(auth: &Auth) -> Value {
    let pair = |key: &str, value: &str| json!({"key": key, "value": value, "type": "string"});
    match auth {
        Auth::None => json!({"type": "noauth"}),
        // Rivet's "inherit" uses the environment's `token` as a bearer token.
        Auth::Inherit => json!({"type": "bearer", "bearer": [pair("token", "{{token}}")]}),
        Auth::Bearer { token } => json!({"type": "bearer", "bearer": [pair("token", token)]}),
        Auth::Basic { username, password } => json!({
            "type": "basic",
            "basic": [pair("username", username), pair("password", password)],
        }),
        Auth::ApiKey { key, value, in_query } => json!({
            "type": "apikey",
            "apikey": [
                pair("key", key),
                pair("value", value),
                pair("in", if *in_query { "query" } else { "header" }),
            ],
        }),
    }
}

/// Postman's structured URL: `raw` plus host/path split, query and `:var` path
/// variables. `{{vars}}` stay as they are (Postman resolves them the same way).
fn pm_url(request: &RequestSpec) -> Value {
    let (base, inline_query) = match request.url.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (request.url.as_str(), None),
    };
    let mut query: Vec<Value> = Vec::new();
    if let Some(q) = inline_query {
        for pair in q.split('&').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            query.push(json!({"key": k, "value": v}));
        }
    }
    query.extend(request.params.iter().filter(|p| !p.key.is_empty() || !p.value.is_empty()).map(pm_kv));
    let raw_query: Vec<String> = query
        .iter()
        .filter(|q| q.get("disabled").is_none())
        .map(|q| format!("{}={}", q["key"].as_str().unwrap_or(""), q["value"].as_str().unwrap_or("")))
        .collect();
    let raw = if raw_query.is_empty() { base.to_string() } else { format!("{base}?{}", raw_query.join("&")) };

    let (protocol, rest) = match base.split_once("://") {
        Some((p, r)) => (Some(p), r),
        None => (None, base),
    };
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let mut url = json!({"raw": raw});
    if let Some(p) = protocol {
        url["protocol"] = json!(p);
    }
    let host_parts: Vec<&str> = if host.contains("{{") { vec![host] } else { host.split('.').collect() };
    url["host"] = json!(host_parts);
    if !path.is_empty() {
        url["path"] = json!(path.split('/').collect::<Vec<_>>());
    }
    if !query.is_empty() {
        url["query"] = Value::Array(query);
    }
    if !request.path_vars.is_empty() {
        url["variable"] = Value::Array(request.path_vars.iter().map(pm_kv).collect());
    }
    url
}

fn pm_body(request: &RequestSpec, warnings: &mut Vec<String>) -> Option<Value> {
    let body = &request.body;
    Some(match body.kind {
        BodyKind::None => return None,
        BodyKind::Json => json!({
            "mode": "raw",
            "raw": body.text,
            "options": {"raw": {"language": "json"}},
        }),
        BodyKind::Text => json!({"mode": "raw", "raw": body.text, "options": {"raw": {"language": "text"}}}),
        BodyKind::Form => json!({"mode": "urlencoded", "urlencoded": body.form.iter().map(pm_kv).collect::<Vec<_>>()}),
        BodyKind::Multipart => json!({
            "mode": "formdata",
            "formdata": body.form.iter().map(|f| {
                let mut v = if f.file {
                    json!({"key": f.key, "type": "file", "src": f.value})
                } else {
                    json!({"key": f.key, "value": f.value, "type": "text"})
                };
                if !f.enabled {
                    v["disabled"] = json!(true);
                }
                if !f.description.is_empty() {
                    v["description"] = json!(f.description);
                }
                v
            }).collect::<Vec<_>>(),
        }),
        BodyKind::GraphQl => json!({
            "mode": "graphql",
            "graphql": {"query": body.text, "variables": body.variables},
        }),
        BodyKind::Binary => {
            let src = body.file_path.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
            if src.is_empty() {
                warnings.push(format!("«{}»: бинарное тело без файла", request.name));
            }
            json!({"mode": "file", "file": {"src": src}})
        }
    })
}

fn pm_events(request: &RequestSpec) -> Vec<Value> {
    let event = |listen: &str, code: &str| {
        json!({
            "listen": listen,
            "script": {"type": "text/javascript", "exec": code.lines().collect::<Vec<_>>()},
        })
    };
    let mut events = Vec::new();
    if !request.scripts.pre_request.trim().is_empty() {
        events.push(event("prerequest", &request.scripts.pre_request));
    }
    if !request.scripts.post_response.trim().is_empty() {
        events.push(event("test", &request.scripts.post_response));
    }
    events
}

fn pm_item(request: &RequestSpec, warnings: &mut Vec<String>) -> Value {
    let mut req = json!({
        "method": request.method.full_label(),
        "header": request.headers.iter().filter(|h| !h.key.is_empty() || !h.value.is_empty()).map(pm_kv).collect::<Vec<_>>(),
        "url": pm_url(request),
        "auth": pm_auth(&request.auth),
    });
    if let Some(body) = pm_body(request, warnings) {
        req["body"] = body;
    }
    if !request.description.is_empty() {
        req["description"] = json!(request.description);
    }
    let mut item = json!({"name": request.name, "request": req.clone()});
    if !request.examples.is_empty() {
        item["response"] = Value::Array(
            request
                .examples
                .iter()
                .map(|e| {
                    let mut original = req.clone();
                    if let Some(url) = &e.url {
                        original["url"] = json!({"raw": url});
                    }
                    let json_body = e
                        .headers
                        .iter()
                        .any(|h| h.key.eq_ignore_ascii_case("content-type") && h.value.contains("json"));
                    json!({
                        "name": e.name,
                        "originalRequest": original,
                        "status": e.status_text,
                        "code": e.status,
                        "_postman_previewlanguage": if json_body { "json" } else { "text" },
                        "header": e.headers.iter().map(pm_kv).collect::<Vec<_>>(),
                        "body": e.body,
                    })
                })
                .collect(),
        );
    }
    let events = pm_events(request);
    if !events.is_empty() {
        item["event"] = Value::Array(events);
    }
    let mut behavior = Map::new();
    if let Some(follow) = request.follow_redirects {
        behavior.insert("followRedirects".into(), json!(follow));
    }
    if let Some(max) = request.max_redirects {
        behavior.insert("maxRedirects".into(), json!(max));
    }
    if let Some(verify) = request.ssl_verify {
        behavior.insert("strictSSL".into(), json!(verify));
    }
    if !request.disabled_auto_headers.is_empty() {
        let off: Map<String, Value> =
            request.disabled_auto_headers.iter().map(|name| (name.clone(), json!(true))).collect();
        behavior.insert("disabledSystemHeaders".into(), Value::Object(off));
    }
    if !behavior.is_empty() {
        item["protocolProfileBehavior"] = Value::Object(behavior);
    }
    item
}

/// Folder tree node while building nested `item` arrays.
#[derive(Default)]
struct Folder {
    /// Children in first-seen order: either a sub-folder name or a request index.
    order: Vec<Entry>,
    folders: BTreeMap<String, Folder>,
}

enum Entry {
    Folder(String),
    Request(usize),
}

impl Folder {
    fn folder(&mut self, path: &str) -> &mut Folder {
        let mut node = self;
        for part in path.split(" / ").map(str::trim).filter(|p| !p.is_empty()) {
            if !node.folders.contains_key(part) {
                node.order.push(Entry::Folder(part.to_string()));
                node.folders.insert(part.to_string(), Folder::default());
            }
            node = node.folders.get_mut(part).unwrap();
        }
        node
    }
}

fn folder_tree(collection: &Collection) -> Folder {
    let mut root = Folder::default();
    for f in &collection.folders {
        root.folder(f);
    }
    for (ix, r) in collection.requests.iter().enumerate() {
        root.folder(r.folder.as_deref().unwrap_or("")).order.push(Entry::Request(ix));
    }
    root
}

pub fn to_postman(collection: &Collection) -> (Value, Vec<String>) {
    let mut warnings = Vec::new();
    fn items(node: &Folder, c: &Collection, warnings: &mut Vec<String>) -> Vec<Value> {
        node.order
            .iter()
            .filter_map(|entry| match entry {
                Entry::Folder(name) => Some(json!({
                    "name": name,
                    "item": items(&node.folders[name], c, warnings),
                })),
                Entry::Request(ix) => {
                    let r = &c.requests[*ix];
                    if r.is_ws() {
                        warnings.push(format!(
                            "«{}»: WebSocket-запросы не входят в формат Postman v2.1 (сохраняются в формате Rivet)",
                            r.name
                        ));
                        return None;
                    }
                    Some(pm_item(r, warnings))
                }
            })
            .collect()
    }
    let tree = folder_tree(collection);
    let mut doc = json!({
        "info": {
            "_postman_id": collection.id,
            "name": collection.name,
            "schema": POSTMAN_SCHEMA,
            "description": collection.description,
        },
        "item": items(&tree, collection, &mut warnings),
    });
    if !collection.vars.is_empty() {
        doc["variable"] = Value::Array(collection.vars.iter().map(pm_kv).collect());
    }
    (doc, warnings)
}

// ---- OpenAPI -----------------------------------------------------------------

/// A JSON schema describing `value` (types, object properties, array items).
fn infer_schema(value: &Value) -> Value {
    match value {
        Value::Null => json!({"nullable": true}),
        Value::Bool(_) => json!({"type": "boolean"}),
        Value::Number(n) if n.is_i64() || n.is_u64() => json!({"type": "integer"}),
        Value::Number(_) => json!({"type": "number"}),
        Value::String(_) => json!({"type": "string"}),
        Value::Array(items) => match items.first() {
            Some(first) => json!({"type": "array", "items": infer_schema(first)}),
            None => json!({"type": "array", "items": {}}),
        },
        Value::Object(map) => {
            let props: Map<String, Value> =
                map.iter().map(|(k, v)| (k.clone(), infer_schema(v))).collect();
            json!({"type": "object", "properties": props})
        }
    }
}

fn form_schema(fields: &[KeyValue], multipart: bool) -> Value {
    let mut props = Map::new();
    for f in fields.iter().filter(|f| f.enabled && !f.key.is_empty()) {
        let mut schema = if multipart && f.file {
            json!({"type": "string", "format": "binary"})
        } else {
            json!({"type": "string", "example": f.value})
        };
        if !f.description.is_empty() {
            schema["description"] = json!(f.description);
        }
        props.insert(f.key.clone(), schema);
    }
    json!({"type": "object", "properties": props})
}

/// Splits a request URL into an optional server and an OpenAPI path template.
/// `{{baseUrl}}/users/{{id}}` → (`{{baseUrl}}`, `/users/{id}`) with `id` as a
/// path variable; `https://h/x` → (`https://h`, `/x`).
pub(crate) fn split_url(url: &str) -> (String, String, Vec<String>) {
    let base = url.split(['?', '#']).next().unwrap_or("");
    let (server, path) = if let Some(rest) = base.strip_prefix("{{") {
        match rest.find("}}") {
            Some(end) => {
                let server = &base[..end + 4];
                (server.to_string(), base[end + 4..].to_string())
            }
            None => (String::new(), base.to_string()),
        }
    } else if let Some((scheme, rest)) = base.split_once("://") {
        let (host, path) = rest.split_once('/').map_or((rest, ""), |(h, p)| (h, p));
        (format!("{scheme}://{host}"), format!("/{path}"))
    } else {
        (String::new(), base.to_string())
    };
    let mut path = if path.starts_with('/') { path } else { format!("/{path}") };
    let mut path_vars = Vec::new();
    for name in vars::referenced_vars(&path) {
        path = path.replace(&format!("{{{{{name}}}}}"), &format!("{{{name}}}"));
        path_vars.push(name);
    }
    // Postman-style `:id` segments.
    let segments: Vec<String> = path
        .split('/')
        .map(|s| match s.strip_prefix(':') {
            Some(name) if !name.is_empty() => {
                path_vars.push(name.to_string());
                format!("{{{name}}}")
            }
            _ => s.to_string(),
        })
        .collect();
    path = segments.join("/");
    if path.len() > 1 {
        path = path.trim_end_matches('/').to_string();
    }
    (server, path, path_vars)
}

fn operation_id(name: &str, used: &mut HashSet<String>) -> String {
    let mut id = String::new();
    let mut upper = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if upper && !id.is_empty() {
                id.push(c.to_ascii_uppercase());
            } else {
                id.push(c);
            }
            upper = false;
        } else {
            upper = true;
        }
    }
    if id.is_empty() || id.starts_with(|c: char| c.is_ascii_digit()) {
        id.insert_str(0, "op");
    }
    let mut unique = id.clone();
    let mut n = 2;
    while !used.insert(unique.clone()) {
        unique = format!("{id}{n}");
        n += 1;
    }
    unique
}

fn param(name: &str, location: &str, kv: Option<&KeyValue>, example: &str) -> Value {
    let mut p = json!({
        "name": name,
        "in": location,
        "schema": {"type": "string"},
    });
    if location == "path" {
        p["required"] = json!(true);
    }
    if !example.is_empty() {
        p["example"] = json!(example);
    }
    if let Some(kv) = kv {
        if !kv.description.is_empty() {
            p["description"] = json!(kv.description);
        }
        if !kv.enabled {
            p["x-rivet-disabled"] = json!(true);
        }
    }
    p
}

fn security_for(auth: &Auth, schemes: &mut Map<String, Value>) -> Option<Value> {
    let (name, scheme) = match auth {
        Auth::None => return Some(json!([])),
        Auth::Bearer { .. } | Auth::Inherit => {
            ("bearerAuth".to_string(), json!({"type": "http", "scheme": "bearer"}))
        }
        Auth::Basic { .. } => ("basicAuth".to_string(), json!({"type": "http", "scheme": "basic"})),
        Auth::ApiKey { key, in_query, .. } => {
            let location = if *in_query { "query" } else { "header" };
            let clean: String = key.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            (
                format!("apiKey{clean}"),
                json!({"type": "apiKey", "in": location, "name": key}),
            )
        }
    };
    schemes.insert(name.clone(), scheme);
    Some(json!([{ name: [] }]))
}

/// `responses` from the saved examples (grouped by status), or a bare 200.
fn responses_for(r: &RequestSpec) -> Value {
    if r.examples.is_empty() {
        return json!({"200": {"description": "OK"}});
    }
    let mut by_code: BTreeMap<String, Vec<&super::types::Example>> = BTreeMap::new();
    for e in &r.examples {
        by_code.entry(e.status.to_string()).or_default().push(e);
    }
    let mut out = Map::new();
    for (code, examples) in by_code {
        let first = examples[0];
        let media = first
            .headers
            .iter()
            .find(|h| h.key.eq_ignore_ascii_case("content-type"))
            .map(|h| h.value.split(';').next().unwrap_or("").trim().to_string())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| "text/plain".into());
        let value = |e: &super::types::Example| {
            serde_json::from_str::<Value>(&e.body).unwrap_or_else(|_| json!(e.body))
        };
        let mut content = Map::new();
        if examples.len() == 1 {
            content.insert("example".into(), value(first));
        } else {
            let named: Map<String, Value> = examples
                .iter()
                .map(|e| (e.name.clone(), json!({"value": value(e)})))
                .collect();
            content.insert("examples".into(), Value::Object(named));
        }
        let description = if first.status_text.is_empty() { first.name.clone() } else { first.status_text.clone() };
        out.insert(code, json!({"description": description, "content": {media: Value::Object(content)}}));
    }
    Value::Object(out)
}

pub fn to_openapi(collection: &Collection) -> (Value, Vec<String>) {
    let mut warnings = Vec::new();
    let mut paths: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    let mut servers: Vec<String> = Vec::new();
    let mut schemes = Map::new();
    let mut used_ids = HashSet::new();
    let mut tags: Vec<String> = Vec::new();
    let var_value = |name: &str| {
        collection.vars.iter().find(|v| v.key == name).map(|v| v.value.clone()).unwrap_or_default()
    };

    for r in &collection.requests {
        if r.is_ws() {
            warnings.push(format!("«{}»: WebSocket-запрос не описывается в OpenAPI", r.name));
            continue;
        }
        let (server, path, path_vars) = split_url(&r.url);
        if !server.is_empty() && !servers.contains(&server) {
            servers.push(server.clone());
        }
        let method = r.method.full_label().to_ascii_lowercase();
        let entry = paths.entry(path.clone()).or_default();
        if entry.contains_key(&method) {
            warnings.push(format!(
                "«{}»: {} {path} уже описан другим запросом — пропущен",
                r.name,
                r.method.full_label()
            ));
            continue;
        }

        let mut parameters: Vec<Value> = Vec::new();
        for name in &path_vars {
            // `:id` takes its Path Variables row; `{{id}}` the collection variable.
            let row = r.path_vars.iter().find(|kv| &kv.key == name);
            let example = row.map_or_else(|| var_value(name), |kv| kv.value.clone());
            parameters.push(param(name, "path", row, &example));
        }
        if let Some((_, q)) = r.url.split_once('?') {
            for pair in q.split('&').filter(|p| !p.is_empty()) {
                let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                parameters.push(param(k, "query", None, v));
            }
        }
        for p in r.params.iter().filter(|p| !p.key.is_empty()) {
            parameters.push(param(&p.key, "query", Some(p), &p.value));
        }
        let mut content_type_header = None;
        for h in r.headers.iter().filter(|h| !h.key.is_empty()) {
            let lower = h.key.to_ascii_lowercase();
            if lower == "content-type" {
                content_type_header = Some(h.value.clone());
                continue;
            }
            if lower == "authorization" && !matches!(r.auth, Auth::None) {
                continue;
            }
            parameters.push(param(&h.key, "header", Some(h), &h.value));
        }

        let mut op = json!({
            "summary": r.name,
            "operationId": operation_id(&r.name, &mut used_ids),
            "responses": responses_for(r),
        });
        if !r.description.is_empty() {
            op["description"] = json!(r.description);
        }
        if let Some(folder) = r.folder.as_ref().filter(|f| !f.is_empty()) {
            op["tags"] = json!([folder]);
            if !tags.contains(folder) {
                tags.push(folder.clone());
            }
        }
        if !parameters.is_empty() {
            op["parameters"] = Value::Array(parameters);
        }

        let body = &r.body;
        let request_body = match body.kind {
            BodyKind::None => None,
            BodyKind::Json => {
                let media = content_type_header.clone().unwrap_or_else(|| "application/json".into());
                let parsed = serde_json::from_str::<Value>(&body.text).ok();
                // `{"id": {{id}}}` isn't JSON until substituted: infer the
                // schema from a masked copy and keep the text as the example.
                let schema_src = parsed
                    .clone()
                    .or_else(|| serde_json::from_str(&vars::mask_placeholders(&body.text, "0")).ok());
                let mut media_obj = Map::new();
                if let Some(src) = &schema_src {
                    media_obj.insert("schema".into(), infer_schema(src));
                }
                if !body.text.trim().is_empty() {
                    media_obj.insert("example".into(), parsed.unwrap_or_else(|| json!(body.text)));
                }
                Some(json!({ media: Value::Object(media_obj) }))
            }
            BodyKind::Text => {
                let media = content_type_header.clone().unwrap_or_else(|| "text/plain".into());
                Some(json!({ media: {"schema": {"type": "string"}, "example": body.text} }))
            }
            BodyKind::Form => Some(json!({
                "application/x-www-form-urlencoded": {"schema": form_schema(&body.form, false)},
            })),
            BodyKind::Multipart => Some(json!({
                "multipart/form-data": {"schema": form_schema(&body.form, true)},
            })),
            BodyKind::GraphQl => Some(json!({
                "application/json": {
                    "schema": {"type": "object", "properties": {
                        "query": {"type": "string"},
                        "variables": {"type": "object"},
                    }},
                    "example": {
                        "query": body.text,
                        "variables": serde_json::from_str::<Value>(&body.variables).unwrap_or_else(|_| json!({})),
                    },
                },
            })),
            BodyKind::Binary => Some(json!({
                "application/octet-stream": {"schema": {"type": "string", "format": "binary"}},
            })),
        };
        if let Some(content) = request_body {
            op["requestBody"] = json!({"content": content});
        }
        if let Some(security) = security_for(&r.auth, &mut schemes) {
            op["security"] = security;
        }

        // What OpenAPI has no place for.
        let disabled_form: Vec<Value> = body
            .form
            .iter()
            .filter(|f| !f.enabled && matches!(body.kind, BodyKind::Form | BodyKind::Multipart))
            .map(pm_kv)
            .collect();
        if !disabled_form.is_empty() {
            op["x-rivet-disabled-form-fields"] = Value::Array(disabled_form);
        }
        if !r.scripts.pre_request.trim().is_empty() || !r.scripts.post_response.trim().is_empty() {
            op["x-rivet-scripts"] = json!({
                "pre_request": r.scripts.pre_request,
                "post_response": r.scripts.post_response,
            });
        }
        if r.follow_redirects.is_some() || r.max_redirects.is_some() {
            op["x-rivet-redirects"] = json!({"follow": r.follow_redirects, "max": r.max_redirects});
        }
        if r.timeout_ms.is_some() || r.ssl_verify.is_some() {
            op["x-rivet-connection"] = json!({"timeout_ms": r.timeout_ms, "ssl_verify": r.ssl_verify});
        }
        if !r.disabled_auto_headers.is_empty() {
            op["x-rivet-disabled-auto-headers"] = json!(r.disabled_auto_headers);
        }
        if body.kind == BodyKind::Binary {
            if let Some(p) = &body.file_path {
                op["x-rivet-body-file"] = json!(p.display().to_string());
            }
        }
        op["x-rivet-request-id"] = json!(r.id);
        entry.insert(method, op);
    }

    let server_list: Vec<Value> = servers
        .iter()
        .map(|s| match s.strip_prefix("{{").and_then(|s| s.strip_suffix("}}")) {
            Some(name) => {
                let default = var_value(name);
                json!({
                    "url": format!("{{{name}}}"),
                    "variables": {name: {"default": if default.is_empty() { "http://localhost".to_string() } else { default }}},
                })
            }
            None => json!({"url": s}),
        })
        .collect();
    let mut doc = json!({
        "openapi": "3.0.3",
        "info": {"title": collection.name, "version": "1.0.0", "description": collection.description},
        "paths": paths.into_iter().map(|(k, v)| (k, Value::Object(v))).collect::<Map<_, _>>(),
    });
    if !server_list.is_empty() {
        doc["servers"] = Value::Array(server_list);
    }
    if !tags.is_empty() {
        doc["tags"] = Value::Array(tags.iter().map(|t| json!({"name": t})).collect());
    }
    if !schemes.is_empty() {
        doc["components"] = json!({"securitySchemes": schemes});
    }
    if !collection.vars.is_empty() {
        doc["x-rivet-variables"] = Value::Array(collection.vars.iter().map(pm_kv).collect());
    }
    (doc, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::openapi::Spec;
    use crate::core::postman::PostmanImport;
    use crate::core::types::{Body, Method, Scripts};

    fn sample() -> Collection {
        let mut c = Collection::new("Shop API");
        c.description = "Shop **API**\nv2".into();
        c.vars = vec![KeyValue::new("baseUrl", "https://api.shop.dev"), KeyValue::new("id", "42")];
        c.folders = vec!["Users".into(), "Users / Admin".into(), "Empty".into()];
        let mut list = RequestSpec::new("List users", Method::Get, "{{baseUrl}}/users?active=1");
        list.folder = Some("Users".into());
        list.params = vec![
            KeyValue::new("limit", "10").description("page size"),
            KeyValue::new("debug", "1").enabled(false),
        ];
        list.headers = vec![KeyValue::new("Accept", "application/json"), KeyValue::new("X-Off", "1").enabled(false)];
        list.auth = Auth::Bearer { token: "{{token}}".into() };
        list.scripts = Scripts { pre_request: "pm.environment.set('a', 1);\nconsole.log(2);".into(), post_response: "pm.test('ok', () => {});".into() };
        list.follow_redirects = Some(false);
        list.max_redirects = Some(3);
        list.ssl_verify = Some(false);
        list.timeout_ms = Some(1500);
        list.disabled_auto_headers = vec!["user-agent".into(), "authorization".into()];
        list.description = "Lists users.\n\n- paged".into();
        list.examples = vec![
            crate::core::types::Example {
                id: "e1".into(),
                name: "Two users".into(),
                status: 200,
                status_text: "OK".into(),
                headers: vec![KeyValue::new("Content-Type", "application/json")],
                body: "[{\"id\":1},{\"id\":2}]".into(),
                url: Some("https://api.shop.dev/users?limit=10".into()),
                saved_at: 0,
            },
            crate::core::types::Example {
                id: "e2".into(),
                name: "Unauthorized".into(),
                status: 401,
                status_text: "Unauthorized".into(),
                headers: vec![],
                body: "nope".into(),
                url: None,
                saved_at: 0,
            },
        ];
        let mut get = RequestSpec::new("Get user", Method::Get, "{{baseUrl}}/users/{{id}}");
        get.folder = Some("Users / Admin".into());
        get.auth = Auth::ApiKey { key: "X-Key".into(), value: "k".into(), in_query: false };
        let mut create = RequestSpec::new("Create user", Method::Post, "{{baseUrl}}/users");
        create.body = Body::json("{\"name\": \"Ann\", \"age\": {{age}}, \"tags\": [\"a\"]}");
        create.auth = Auth::Basic { username: "u".into(), password: "p".into() };
        let mut login = RequestSpec::new("Login", Method::Post, "https://auth.example.com/login");
        login.body = Body { kind: BodyKind::Form, form: vec![KeyValue::new("user", "a"), KeyValue::new("pass", "b").enabled(false)], ..Body::default() };
        let mut upload = RequestSpec::new("Upload", Method::Put, "{{baseUrl}}/files/:name");
        upload.path_vars = vec![KeyValue::new("name", "logo.png").description("file name")];
        let mut file = KeyValue::new("file", "/tmp/a.png");
        file.file = true;
        upload.body = Body { kind: BodyKind::Multipart, form: vec![file, KeyValue::new("note", "hi").description("caption")], ..Body::default() };
        let ws = RequestSpec::untitled_ws();
        for mut r in [list, get, create, login, upload, ws] {
            r.collection_id = Some(c.id.clone());
            c.requests.push(r);
        }
        c
    }

    #[test]
    fn rivet_round_trip_is_lossless() {
        let c = sample();
        let text = export(&c, ExportFormat::Rivet).unwrap().text;
        let doc: Value = serde_json::from_str(&text).unwrap();
        assert!(is_rivet(&doc));
        let back = from_rivet(&doc).unwrap();
        assert_ne!(back.id, c.id, "fresh ids");
        let strip = |c: &Collection| {
            let mut c = c.clone();
            c.id.clear();
            for r in &mut c.requests {
                r.id.clear();
                r.collection_id = None;
            }
            c
        };
        assert_eq!(strip(&back), strip(&c));
        assert!(back.requests.iter().all(|r| r.collection_id.as_deref() == Some(back.id.as_str())));
    }

    #[test]
    fn postman_export_round_trips_through_the_importer() {
        let c = sample();
        let out = export(&c, ExportFormat::Postman).unwrap();
        assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
        assert!(out.warnings[0].contains("WebSocket"));
        let doc: Value = serde_json::from_str(&out.text).unwrap();
        assert_eq!(doc["info"]["schema"], POSTMAN_SCHEMA);
        // Nested folders, empty ones included.
        assert_eq!(doc["item"][0]["name"], "Users");
        assert_eq!(doc["item"][0]["item"][0]["name"], "Admin");
        assert_eq!(doc["item"][1]["name"], "Empty");
        assert_eq!(doc["item"][0]["item"][1]["protocolProfileBehavior"], json!({"followRedirects": false, "maxRedirects": 3, "strictSSL": false, "disabledSystemHeaders": {"user-agent": true, "authorization": true}}));

        let imported = PostmanImport::parse(&out.text).unwrap().collection;
        assert_eq!(imported.name, "Shop API");
        assert_eq!(imported.vars, c.vars);
        assert_eq!(imported.description, c.description);
        let mut folders = imported.folders.clone();
        folders.sort();
        assert_eq!(folders, ["Empty", "Users", "Users / Admin"]);
        let by_name = |n: &str| imported.requests.iter().find(|r| r.name == n).unwrap();
        let list = by_name("List users");
        assert_eq!(list.url, "{{baseUrl}}/users");
        assert_eq!(
            list.params,
            vec![
                KeyValue::new("active", "1"),
                KeyValue::new("limit", "10").description("page size"),
                KeyValue::new("debug", "1").enabled(false),
            ]
        );
        assert_eq!(list.headers, c.requests[0].headers);
        assert_eq!(list.auth, c.requests[0].auth);
        assert_eq!(list.scripts, c.requests[0].scripts);
        assert_eq!(list.description, c.requests[0].description);
        assert_eq!(list.folder.as_deref(), Some("Users"));
        assert_eq!(list.follow_redirects, Some(false));
        assert_eq!(list.ssl_verify, Some(false));
        assert_eq!(list.disabled_auto_headers, ["user-agent", "authorization"]);
        let strip = |e: &crate::core::types::Example| (e.name.clone(), e.status, e.status_text.clone(), e.headers.clone(), e.body.clone());
        assert_eq!(
            list.examples.iter().map(strip).collect::<Vec<_>>(),
            c.requests[0].examples.iter().map(strip).collect::<Vec<_>>()
        );
        assert_eq!(list.examples[0].url.as_deref(), Some("https://api.shop.dev/users?limit=10"));
        assert_eq!(list.max_redirects, Some(3));
        assert_eq!(by_name("Get user").auth, c.requests[1].auth);
        assert_eq!(by_name("Get user").folder.as_deref(), Some("Users / Admin"));
        let create = by_name("Create user");
        assert_eq!(create.body, c.requests[2].body);
        assert_eq!(create.auth, c.requests[2].auth);
        assert_eq!(by_name("Login").body, c.requests[3].body);
        assert_eq!(by_name("Login").url, "https://auth.example.com/login");
        assert_eq!(by_name("Upload").body, c.requests[4].body);
        assert_eq!(by_name("Upload").method, Method::Put);
        assert_eq!(by_name("Upload").url, "{{baseUrl}}/files/:name");
        assert_eq!(by_name("Upload").path_vars, c.requests[4].path_vars);
    }

    #[test]
    fn openapi_export_is_a_loadable_spec() {
        let c = sample();
        let out = export(&c, ExportFormat::OpenApi).unwrap();
        assert!(out.warnings.iter().any(|w| w.contains("WebSocket")));
        let spec = Spec::parse(&out.text).unwrap();
        assert_eq!(spec.title, "Shop API");
        let endpoints: Vec<(Method, String)> = spec
            .groups
            .iter()
            .flat_map(|g| g.endpoints.iter().map(|e| (e.method, e.path.clone())))
            .collect();
        for expected in [
            (Method::Get, "/users"),
            (Method::Get, "/users/{id}"),
            (Method::Post, "/users"),
            (Method::Post, "/login"),
            (Method::Put, "/files/{name}"),
        ] {
            assert!(endpoints.iter().any(|(m, p)| *m == expected.0 && p == expected.1), "{expected:?} in {endpoints:?}");
        }

        let doc: Value = serde_yaml::from_str(&out.text).unwrap();
        assert_eq!(doc["servers"][0]["url"], "{baseUrl}");
        assert_eq!(doc["servers"][0]["variables"]["baseUrl"]["default"], "https://api.shop.dev");
        assert_eq!(doc["servers"][1]["url"], "https://auth.example.com");
        let list = &doc["paths"]["/users"]["get"];
        assert_eq!(list["tags"], json!(["Users"]));
        assert_eq!(list["description"], "Lists users.\n\n- paged");
        assert_eq!(doc["info"]["description"], "Shop **API**\nv2");
        let params = list["parameters"].as_array().unwrap();
        let names: Vec<&str> = params.iter().map(|p| p["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["active", "limit", "debug", "Accept", "X-Off"]);
        assert_eq!(params[1]["description"], "page size");
        assert_eq!(params[2]["x-rivet-disabled"], true);
        assert_eq!(list["security"], json!([{"bearerAuth": []}]));
        assert_eq!(list["x-rivet-redirects"], json!({"follow": false, "max": 3}));
        assert_eq!(list["x-rivet-connection"], json!({"timeout_ms": 1500, "ssl_verify": false}));
        assert_eq!(list["x-rivet-disabled-auto-headers"], json!(["user-agent", "authorization"]));
        assert_eq!(list["responses"]["200"]["content"]["application/json"]["example"], json!([{"id": 1}, {"id": 2}]));
        assert_eq!(list["responses"]["401"]["content"]["text/plain"]["example"], "nope");
        assert_eq!(list["responses"]["401"]["description"], "Unauthorized");
        assert!(list["x-rivet-scripts"]["pre_request"].as_str().unwrap().contains("console.log"));
        let get = &doc["paths"]["/users/{id}"]["get"];
        assert_eq!(get["parameters"][0], json!({"name": "id", "in": "path", "required": true, "schema": {"type": "string"}, "example": "42"}));
        assert_eq!(get["security"], json!([{"apiKeyXKey": []}]));
        let create = &doc["paths"]["/users"]["post"]["requestBody"]["content"]["application/json"];
        assert_eq!(create["schema"]["properties"]["age"]["type"], "integer");
        assert_eq!(create["schema"]["properties"]["tags"]["items"]["type"], "string");
        assert_eq!(create["example"], json!(c.requests[2].body.text));
        let upload = &doc["paths"]["/files/{name}"]["put"]["requestBody"]["content"]["multipart/form-data"]["schema"];
        assert_eq!(upload["properties"]["file"]["format"], "binary");
        let name = &doc["paths"]["/files/{name}"]["put"]["parameters"][0];
        assert_eq!(name["example"], "logo.png");
        assert_eq!(name["description"], "file name");
        assert_eq!(upload["properties"]["note"]["description"], "caption");
        assert_eq!(doc["paths"]["/login"]["post"]["x-rivet-disabled-form-fields"][0]["key"], "pass");
        assert_eq!(doc["components"]["securitySchemes"]["basicAuth"], json!({"type": "http", "scheme": "basic"}));
        assert_eq!(doc["x-rivet-variables"][1], json!({"key": "id", "value": "42"}));
    }

    #[test]
    fn rivet_file_imports_through_import_file() {
        let c = sample();
        let dir = std::env::temp_dir().join(format!("rivet-export-{}", crate::core::types::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(ExportFormat::Rivet.file_name(&c.name));
        std::fs::write(&path, export(&c, ExportFormat::Rivet).unwrap().text).unwrap();
        let outcome = crate::core::import::import_file(&path).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(outcome.format, "rivet");
        assert_eq!(outcome.collection.requests.len(), c.requests.len());
        assert!(outcome.collection.requests.iter().any(|r| r.is_ws()), "WebSocket kept");
        assert_eq!(outcome.collection.folders, c.folders);
    }

    #[test]
    fn graphql_round_trips_through_postman() {
        let mut c = Collection::new("G");
        let mut q = RequestSpec::new("Users", Method::Post, "https://h/graphql");
        q.body = Body { kind: BodyKind::GraphQl, text: "{ users { id } }".into(), variables: "{\"n\": 1}".into(), ..Body::default() };
        c.requests.push(q);
        let (doc, _) = to_postman(&c);
        assert_eq!(doc["item"][0]["request"]["body"]["mode"], "graphql");
        let back = PostmanImport::parse(&doc.to_string()).unwrap().collection;
        assert_eq!(back.requests[0].body, c.requests[0].body);
        let (oas, _) = to_openapi(&c);
        let example = &oas["paths"]["/graphql"]["post"]["requestBody"]["content"]["application/json"]["example"];
        assert_eq!(example, &json!({"query": "{ users { id } }", "variables": {"n": 1}}));
    }

    #[test]
    fn openapi_duplicates_and_names() {
        let mut c = Collection::new("Dup");
        c.requests.push(RequestSpec::new("A", Method::Get, "http://h/x"));
        c.requests.push(RequestSpec::new("A", Method::Get, "http://h/x/"));
        c.requests.push(RequestSpec::new("A", Method::Post, "http://h/x"));
        let (doc, warnings) = to_openapi(&c);
        assert_eq!(warnings.len(), 1);
        assert_eq!(doc["paths"]["/x"]["get"]["operationId"], "A");
        assert_eq!(doc["paths"]["/x"]["post"]["operationId"], "A2");
        assert_eq!(ExportFormat::OpenApi.file_name("Мой API / v2"), "Мой_API___v2.openapi.yaml");
        assert_eq!(ExportFormat::Postman.file_name("///"), "collection.postman_collection.json");
        assert_eq!(ExportFormat::from_id("POSTMAN"), Some(ExportFormat::Postman));
    }
}
