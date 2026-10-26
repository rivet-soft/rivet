//! OpenAPI 3.x and Swagger 2.0 import.
//!
//! Parses YAML or JSON, groups operations by their first tag (document order,
//! untagged → `default`), builds example request bodies and query params, and
//! answers schema questions about responses (label, validation).

use std::path::Path;

use serde_json::{Map, Value};

use crate::core::schema;
use crate::core::types::{
    Body, BodyKind, KeyValue, Method, Mismatch, SpecEndpoint, SpecGroup, SpecRef,
};

/// Request body synthesized from an operation's declared media type and schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedRequestBody {
    pub body: Body,
    pub content_type: Option<String>,
}

const METHODS: [(&str, Method); 7] = [
    ("get", Method::Get),
    ("post", Method::Post),
    ("put", Method::Put),
    ("patch", Method::Patch),
    ("delete", Method::Delete),
    ("head", Method::Head),
    ("options", Method::Options),
];

#[derive(Clone, Debug)]
pub struct Spec {
    pub title: String,
    pub version: String,
    pub servers: Vec<String>,
    pub groups: Vec<SpecGroup>,
    /// Whole document as JSON, kept for `$ref` resolution.
    doc: Value,
    swagger2: bool,
}

impl Spec {
    /// Read and parse a spec file (YAML or JSON).
    pub fn load(path: &Path) -> Result<Spec, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("Не удалось прочитать {}: {e}", path.display()))?;
        let parsed = Spec::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let resolved = super::openapi_refs::resolve(path, parsed.document(), None)?;
        Spec::from_value(resolved.document)
    }

    /// Parse an OpenAPI 3.x or Swagger 2.0 document.
    pub fn parse(text: &str) -> Result<Spec, String> {
        let text = text.trim_start_matches('\u{feff}');
        let doc: Value = if text.trim_start().starts_with('{') {
            serde_json::from_str(text).map_err(|e| format!("Некорректный JSON: {e}"))?
        } else {
            let y: serde_yaml::Value =
                serde_yaml::from_str(text).map_err(|e| format!("Некорректный YAML: {e}"))?;
            yaml_to_json(y)
        };
        Spec::from_value(doc)
    }

    /// Build from an already parsed document.
    pub fn from_value(doc: Value) -> Result<Spec, String> {
        if !doc.is_object() {
            return Err("Спецификация должна быть объектом".into());
        }
        let swagger2 = doc.get("swagger").is_some();
        let version = if swagger2 {
            doc.get("swagger")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        } else {
            match doc.get("openapi") {
                Some(Value::String(v)) => v.clone(),
                Some(v @ Value::Number(_)) => v.to_string(),
                _ => return Err("Не найдено поле `openapi` — это не OpenAPI-спецификация".into()),
            }
        };
        if (swagger2 && version != "2.0") || (!swagger2 && !version.starts_with('3')) {
            return Err(format!(
                "Неподдерживаемая версия спецификации {version} (OpenAPI 3.x или Swagger 2.0)"
            ));
        }
        if (swagger2 && !doc.get("paths").is_some_and(Value::is_object))
            || (!swagger2 && !doc.get("paths").is_none_or(Value::is_object))
        {
            return Err("Поле `paths` должно быть объектом".into());
        }

        let info = doc.get("info");
        let title = info
            .and_then(|i| i.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("API")
            .to_string();
        let api_version = match info.and_then(|i| i.get("version")) {
            Some(Value::String(s)) => s.clone(),
            Some(v @ Value::Number(_)) => v.to_string(),
            _ => String::new(),
        };
        let mut servers: Vec<String> = doc
            .get("servers")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.get("url").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if swagger2 {
            if let Some(host) = doc.get("host").and_then(Value::as_str) {
                let base = doc.get("basePath").and_then(Value::as_str).unwrap_or("");
                let schemes: Vec<&str> = doc
                    .get("schemes")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                for scheme in if schemes.is_empty() {
                    vec!["https"]
                } else {
                    schemes
                } {
                    servers.push(format!("{scheme}://{host}{base}"));
                }
            }
        }

        let mut spec = Spec {
            title,
            version: api_version,
            servers,
            groups: Vec::new(),
            doc,
            swagger2,
        };
        spec.groups = spec.build_groups();
        Ok(spec)
    }

    /// The raw document (JSON form).
    pub fn document(&self) -> &Value {
        &self.doc
    }

    /// Default variables for a collection imported from this specification.
    pub fn collection_variables(&self) -> Vec<KeyValue> {
        let Some(server) = self
            .servers
            .iter()
            .find(|s| s.starts_with("http://") || s.starts_with("https://"))
        else {
            return Vec::new();
        };
        let mut url = server.clone();
        let mut variables = Vec::new();
        if let Some(server_doc) =
            self.doc
                .get("servers")
                .and_then(Value::as_array)
                .and_then(|servers| {
                    servers
                        .iter()
                        .find(|s| s.get("url").and_then(Value::as_str) == Some(server))
                })
        {
            if let Some(map) = server_doc.get("variables").and_then(Value::as_object) {
                for (name, variable) in map {
                    if let Some(default) = variable.get("default") {
                        let value = default
                            .as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| default.to_string());
                        url = url.replace(&format!("{{{name}}}"), &format!("{{{{{name}}}}}"));
                        variables.push(KeyValue::new(name, value));
                    }
                }
            }
        }
        let mut result = vec![KeyValue::new("baseUrl", url)];
        result.extend(variables);
        result
    }

    /// All endpoints in group order.
    pub fn endpoints(&self) -> impl Iterator<Item = &SpecEndpoint> {
        self.groups.iter().flat_map(|g| g.endpoints.iter())
    }

    /// Endpoint by method and path. Accepts the path template (`/orders/{id}`) or a
    /// concrete path (`/orders/ord_1`, query string ignored).
    pub fn find(&self, method: Method, path: &str) -> Option<&SpecEndpoint> {
        let path = path.split(['?', '#']).next().unwrap_or(path);
        self.endpoints()
            .find(|e| e.method == method && e.path == path)
            .or_else(|| {
                self.endpoints()
                    .find(|e| e.method == method && template_matches(&e.path, path))
            })
    }

    /// Example JSON body for the operation's request.
    pub fn example_body(&self, r: &SpecRef) -> Option<String> {
        self.find(r.method, &r.path)?.example_body.clone()
    }

    /// Generate the editor body format and example fields for a linked operation.
    pub fn generate_request_body(&self, r: &SpecRef) -> Option<GeneratedRequestBody> {
        let op = self.operation(r)?;
        if self.swagger2 {
            let path_item = self.doc.get("paths")?.get(&r.path);
            let parameters = path_item
                .and_then(|item| item.get("parameters"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .chain(
                    op.get("parameters")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten(),
                )
                .map(|p| self.deref(p));
            let mut form_fields = Vec::new();
            let mut body_param = None;
            for param in parameters {
                match param.get("in").and_then(Value::as_str) {
                    Some("formData") => {
                        let name = param.get("name").and_then(Value::as_str).unwrap_or("");
                        if !name.is_empty() {
                            let mut field = KeyValue::new(
                                name,
                                param
                                    .get("default")
                                    .or_else(|| param.get("example"))
                                    .map(example_field_text)
                                    .unwrap_or_default(),
                            );
                            field.file = param.get("type").and_then(Value::as_str) == Some("file");
                            form_fields.push(field);
                        }
                    }
                    Some("body") => body_param = Some(param),
                    _ => {}
                }
            }
            let consumes = op
                .get("consumes")
                .or_else(|| self.doc.get("consumes"))
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_str)
                .unwrap_or(if form_fields.iter().any(|field| field.file) {
                    "multipart/form-data"
                } else if !form_fields.is_empty() {
                    "application/x-www-form-urlencoded"
                } else {
                    "application/json"
                });
            if !form_fields.is_empty() {
                let kind =
                    if consumes.starts_with("multipart/") || form_fields.iter().any(|f| f.file) {
                        BodyKind::Multipart
                    } else {
                        BodyKind::Form
                    };
                return Some(GeneratedRequestBody {
                    body: Body {
                        kind,
                        text: String::new(),
                        form: form_fields,
                        file_path: None,
                        ..Body::default()
                    },
                    content_type: (kind != BodyKind::Multipart).then(|| consumes.to_string()),
                });
            }
            let param = body_param?;
            let value = param.get("example").cloned().or_else(|| {
                param
                    .get("schema")
                    .map(|s| schema::example_from_schema(&self.doc, s))
            });
            return Some(self.generated_body_for_media(consumes, value, param.get("schema")));
        }
        let request_body = self.deref(op.get("requestBody")?);
        let content = request_body.get("content")?.as_object()?;
        let (media_type, media) = content
            .iter()
            .find(|(name, _)| body_kind_for_media(name) == BodyKind::Json)
            .or_else(|| content.iter().next())?;
        let value = media_example(&self.doc, media).or_else(|| {
            media
                .get("schema")
                .map(|s| schema::example_from_schema(&self.doc, s))
        });
        Some(self.generated_body_for_media(media_type, value, media.get("schema")))
    }

    fn generated_body_for_media(
        &self,
        media_type: &str,
        value: Option<Value>,
        schema_node: Option<&Value>,
    ) -> GeneratedRequestBody {
        let kind = body_kind_for_media(media_type);
        let mut body = Body {
            kind,
            text: String::new(),
            form: Vec::new(),
            file_path: None,
            ..Body::default()
        };
        match kind {
            BodyKind::Json => {
                body.text = value
                    .as_ref()
                    .and_then(|v| serde_json::to_string_pretty(v).ok())
                    .unwrap_or_else(|| "{}".into())
            }
            BodyKind::Text => {
                body.text = value.as_ref().map(example_field_text).unwrap_or_default()
            }
            BodyKind::Form | BodyKind::Multipart => {
                let values = value.as_ref().and_then(Value::as_object);
                let properties = schema_node
                    .map(|s| self.deref(s))
                    .and_then(|s| s.get("properties"))
                    .and_then(Value::as_object);
                if let Some(properties) = properties {
                    for (name, property) in properties {
                        let property = self.deref(property);
                        let mut field = KeyValue::new(
                            name,
                            values
                                .and_then(|v| v.get(name))
                                .map(example_field_text)
                                .unwrap_or_default(),
                        );
                        field.file = kind == BodyKind::Multipart && schema_is_binary(property);
                        if field.file {
                            field.value.clear();
                        }
                        body.form.push(field);
                    }
                }
                if let Some(values) = values {
                    for (name, value) in values {
                        if !body.form.iter().any(|field| field.key == *name) {
                            body.form
                                .push(KeyValue::new(name, example_field_text(value)));
                        }
                    }
                }
            }
            BodyKind::Binary | BodyKind::None | BodyKind::GraphQl => {}
        }
        GeneratedRequestBody {
            body,
            content_type: (kind != BodyKind::Multipart).then(|| media_type.to_string()),
        }
    }

    /// `"responses.201 → $ref Order"` for the response schema matching `status`
    /// (falls back to `2XX`-style ranges and `default`). `None` without a schema.
    pub fn response_schema_label(&self, r: &SpecRef, status: u16) -> Option<String> {
        let (key, schema) = self.response_schema(r, status)?;
        Some(format!(
            "responses.{key} → {}",
            schema::schema_label(&self.doc, schema)
        ))
    }

    /// Validate a response body. `None` when the spec has no schema for it.
    pub fn validate(&self, r: &SpecRef, status: u16, body: &Value) -> Option<Vec<Mismatch>> {
        let (_, schema) = self.response_schema(r, status)?;
        Some(schema::validate(&self.doc, schema, body))
    }

    /// Example value synthesized from the response schema ("Generate from schema").
    pub fn example_response(&self, r: &SpecRef, status: u16) -> Option<String> {
        let (key, schema) = self.response_schema(r, status)?;
        let op = self.operation(r)?;
        let response = self.deref(op.get("responses")?.get(key.as_str())?);
        let value = if self.swagger2 {
            response
                .get("examples")
                .and_then(|e| e.get("application/json"))
                .cloned()
        } else {
            response
                .get("content")
                .and_then(json_media)
                .and_then(|media| media_example(&self.doc, media))
        }
        .unwrap_or_else(|| schema::example_from_schema(&self.doc, schema));
        serde_json::to_string_pretty(&value).ok()
    }

    // ---------------------------------------------------------------------

    fn deref<'a>(&'a self, v: &'a Value) -> &'a Value {
        schema::deref(&self.doc, v)
    }

    /// Operation object for a spec ref (exact template, then template matching).
    fn operation(&self, r: &SpecRef) -> Option<&Value> {
        let paths = self.doc.get("paths")?.as_object()?;
        let path = r.path.split(['?', '#']).next().unwrap_or(&r.path);
        let item = paths.get(path).or_else(|| {
            paths
                .iter()
                .find(|(t, _)| template_matches(t, path))
                .map(|(_, v)| v)
        })?;
        let item = self.deref(item);
        let name = METHODS.iter().find(|(_, m)| *m == r.method)?.0;
        item.get(name).filter(|v| v.is_object())
    }

    /// (matched response key, schema) for a status code.
    fn response_schema(&self, r: &SpecRef, status: u16) -> Option<(String, &Value)> {
        let responses = self.operation(r)?.get("responses")?.as_object()?;
        let exact = status.to_string();
        let range = format!("{}XX", status / 100);
        let key = responses
            .keys()
            .find(|k| **k == exact)
            .or_else(|| responses.keys().find(|k| k.eq_ignore_ascii_case(&range)))
            .or_else(|| responses.keys().find(|k| *k == "default"))?;
        let response = self.deref(&responses[key]);
        let schema = if self.swagger2 {
            response.get("schema")?
        } else {
            json_media(response.get("content")?)?.get("schema")?
        };
        Some((key.clone(), schema))
    }

    fn build_groups(&self) -> Vec<SpecGroup> {
        let mut groups: Vec<SpecGroup> = Vec::new();
        let Some(paths) = self.doc.get("paths").and_then(Value::as_object) else {
            return groups;
        };
        let global_security = self.doc.get("security");
        for (path, item) in paths {
            let item = self.deref(item);
            let Some(item_obj) = item.as_object() else {
                continue;
            };
            let shared_params = item_obj.get("parameters").and_then(Value::as_array);
            for (name, method) in METHODS {
                let Some(op) = item_obj.get(name).filter(|o| o.is_object()) else {
                    continue;
                };
                let tag = op
                    .get("tags")
                    .and_then(Value::as_array)
                    .and_then(|t| t.first())
                    .and_then(Value::as_str)
                    .unwrap_or("default")
                    .to_string();
                let endpoint = SpecEndpoint {
                    method,
                    path: path.clone(),
                    name: endpoint_name(op, method, path),
                    description: op
                        .get("description")
                        .or_else(|| item_obj.get("description"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_string(),
                    params: self.query_params(
                        shared_params,
                        op.get("parameters").and_then(Value::as_array),
                    ),
                    example_body: self.request_example(op, shared_params),
                    requires_auth: requires_auth(op.get("security").or(global_security)),
                };
                match groups.iter_mut().find(|g| g.name == tag) {
                    Some(g) => g.endpoints.push(endpoint),
                    None => groups.push(SpecGroup {
                        name: tag,
                        endpoints: vec![endpoint],
                    }),
                }
            }
        }
        groups
    }

    /// Query parameters (path-level + operation-level, operation overrides).
    fn query_params(&self, shared: Option<&Vec<Value>>, own: Option<&Vec<Value>>) -> Vec<KeyValue> {
        let mut merged: Vec<&Value> = Vec::new();
        for p in shared
            .into_iter()
            .flatten()
            .chain(own.into_iter().flatten())
        {
            let p = self.deref(p);
            let key = (p.get("name"), p.get("in"));
            merged.retain(|q| (q.get("name"), q.get("in")) != key);
            merged.push(p);
        }
        merged
            .into_iter()
            .filter(|p| p.get("in").and_then(Value::as_str) == Some("query"))
            .filter_map(|p| {
                let name = p.get("name")?.as_str()?;
                let required = p.get("required").and_then(Value::as_bool).unwrap_or(false);
                let desc = p
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                Some(
                    KeyValue::new(name, self.param_value(p))
                        .description(desc)
                        .enabled(required),
                )
            })
            .collect()
    }

    fn param_value(&self, p: &Value) -> String {
        let value = p
            .get("example")
            .cloned()
            .or_else(|| {
                p.get("examples")
                    .and_then(Value::as_object)
                    .and_then(|m| m.values().next())
                    .and_then(|e| self.deref(e).get("value").cloned())
            })
            .or_else(|| {
                let s = self.deref(p.get("schema")?);
                s.get("example")
                    .or_else(|| {
                        s.get("examples")
                            .and_then(Value::as_array)
                            .and_then(|a| a.first())
                    })
                    .or_else(|| s.get("default"))
                    .or_else(|| {
                        s.get("enum")
                            .and_then(Value::as_array)
                            .and_then(|a| a.first())
                    })
                    .cloned()
            })
            .or_else(|| p.get("default").cloned())
            .or_else(|| {
                p.get("enum")
                    .and_then(Value::as_array)
                    .and_then(|a| a.first())
                    .cloned()
            });
        match value {
            Some(Value::String(s)) => s,
            Some(Value::Null) | None => String::new(),
            Some(Value::Array(a)) => a
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| v.to_string())
                })
                .collect::<Vec<_>>()
                .join(","),
            Some(v) => v.to_string(),
        }
    }

    fn request_example(&self, op: &Value, shared: Option<&Vec<Value>>) -> Option<String> {
        if self.swagger2 {
            let body = shared
                .into_iter()
                .flatten()
                .chain(
                    op.get("parameters")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten(),
                )
                .map(|p| self.deref(p))
                .find(|p| p.get("in").and_then(Value::as_str) == Some("body"))?;
            let value = body.get("example").cloned().or_else(|| {
                body.get("schema")
                    .map(|s| schema::example_from_schema(&self.doc, s))
            })?;
            return serde_json::to_string_pretty(&value).ok();
        }
        let body = self.deref(op.get("requestBody")?);
        let media = json_media(body.get("content")?)?;
        let value = media_example(&self.doc, media).or_else(|| {
            media
                .get("schema")
                .map(|s| schema::example_from_schema(&self.doc, s))
        })?;
        serde_json::to_string_pretty(&value).ok()
    }
}

fn body_kind_for_media(media_type: &str) -> BodyKind {
    let media_type = media_type
        .split(';')
        .next()
        .unwrap_or(media_type)
        .trim()
        .to_ascii_lowercase();
    if media_type == "multipart/form-data" {
        BodyKind::Multipart
    } else if media_type == "application/x-www-form-urlencoded" {
        BodyKind::Form
    } else if media_type.contains("json") {
        BodyKind::Json
    } else if media_type.starts_with("text/") || media_type.contains("xml") {
        BodyKind::Text
    } else {
        BodyKind::Binary
    }
}

fn example_field_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn schema_is_binary(schema: &Value) -> bool {
    schema.get("format").and_then(Value::as_str) == Some("binary")
        || schema.get("type").and_then(Value::as_str) == Some("file")
        || schema.get("items").is_some_and(schema_is_binary)
}

fn endpoint_name(op: &Value, method: Method, path: &str) -> String {
    let pick = |k: &str| {
        op.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    pick("summary")
        .or_else(|| pick("operationId"))
        .map(str::to_string)
        .unwrap_or_else(|| format!("{} {path}", method.full_label()))
}

/// `security` → requires auth: a non-empty list without an anonymous (`{}`) alternative.
fn requires_auth(security: Option<&Value>) -> bool {
    match security.and_then(Value::as_array) {
        Some(reqs) => {
            !reqs.is_empty()
                && reqs
                    .iter()
                    .all(|r| r.as_object().is_some_and(|o| !o.is_empty()))
        }
        None => false,
    }
}

/// Pick the JSON media type from a `content` map: `application/json`, then any
/// `*json*` type, then `*/*`.
fn json_media(content: &Value) -> Option<&Value> {
    let map = content.as_object()?;
    map.get("application/json")
        .or_else(|| map.iter().find(|(k, _)| k.contains("json")).map(|(_, v)| v))
        .or_else(|| map.get("*/*"))
}

/// `example` or first of `examples` on a media type object.
fn media_example(doc: &Value, media: &Value) -> Option<Value> {
    if let Some(e) = media.get("example") {
        return Some(e.clone());
    }
    let first = media.get("examples")?.as_object()?.values().next()?;
    schema::deref(doc, first).get("value").cloned()
}

/// `/orders/{id}` matches `/orders/ord_1`.
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

/// serde_yaml → serde_json, stringifying non-string keys (`200:` → `"200"`).
pub(crate) fn yaml_to_json(y: serde_yaml::Value) -> Value {
    use serde_yaml::Value as Y;
    match y {
        Y::Null => Value::Null,
        Y::Bool(b) => Value::Bool(b),
        Y::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::from(i)
            } else if let Some(u) = n.as_u64() {
                Value::from(u)
            } else {
                n.as_f64()
                    .and_then(serde_json::Number::from_f64)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            }
        }
        Y::String(s) => Value::String(s),
        Y::Sequence(seq) => Value::Array(seq.into_iter().map(yaml_to_json).collect()),
        Y::Mapping(m) => {
            let mut out = Map::new();
            for (k, v) in m {
                let key = match k {
                    Y::String(s) => s,
                    Y::Number(n) => n.to_string(),
                    Y::Bool(b) => b.to_string(),
                    Y::Null => "null".into(),
                    other => serde_yaml::to_string(&other)
                        .unwrap_or_default()
                        .trim()
                        .to_string(),
                };
                out.insert(key, yaml_to_json(v));
            }
            Value::Object(out)
        }
        Y::Tagged(t) => yaml_to_json(t.value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unquoted_status_keys_and_untagged() {
        let spec = Spec::parse(
            "openapi: 3.1.0\ninfo: {title: T, version: 1}\npaths:\n  /x:\n    get:\n      responses:\n        200:\n          description: ok\n          content:\n            application/json:\n              schema: {type: [string, 'null']}\n",
        )
        .unwrap();
        assert_eq!(spec.version, "1");
        assert_eq!(spec.groups[0].name, "default");
        assert_eq!(spec.groups[0].endpoints[0].name, "GET /x");
        let r = SpecRef {
            method: Method::Get,
            path: "/x".into(),
        };
        assert_eq!(spec.validate(&r, 200, &Value::Null), Some(vec![]));
        assert_eq!(
            spec.response_schema_label(&r, 200).as_deref(),
            Some("responses.200 → string | null")
        );
    }

    #[test]
    fn rejects_non_openapi() {
        assert!(Spec::parse("swagger: '2.0'").is_err());
        assert!(Spec::parse("{\"foo\": 1}").is_err());
        assert!(Spec::parse("{ not json").is_err());
    }

    #[test]
    fn template_matching() {
        assert!(template_matches("/orders/{id}", "/orders/42"));
        assert!(!template_matches("/orders/{id}", "/orders"));
        assert!(!template_matches("/orders", "/orders"));
    }
}
