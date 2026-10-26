//! Import Postman Collection v2.0/v2.1 JSON into Relay's collection model.

use std::path::Path;

use serde_json::Value;

use super::types::{Auth, Body, BodyKind, Collection, KeyValue, Method, RequestSpec, Scripts};

pub struct PostmanImport {
    pub collection: Collection,
    pub skipped: usize,
    pub unsupported_bodies: usize,
    pub unsupported_auth: usize,
}

impl PostmanImport {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let doc: Value = serde_json::from_str(text)
            .map_err(|e| format!("Некорректный JSON коллекции Postman: {e}"))?;
        if !is_postman(&doc) {
            return Err("Это не коллекция Postman v2.0/v2.1".into());
        }
        let name = doc["info"]["name"].as_str().unwrap_or("Postman collection");
        let mut collection = Collection::new(name);
        collection.vars = key_values(doc.get("variable"), "key");
        let mut out = Self {
            collection,
            skipped: 0,
            unsupported_bodies: 0,
            unsupported_auth: 0,
        };
        let auth = doc.get("auth");
        out.walk(doc["item"].as_array().unwrap(), "", auth);
        Ok(out)
    }

    /// Resolve relative file entries against the exported collection's directory.
    pub fn resolve_file_paths(&mut self, collection_path: &Path) {
        let Some(base) = collection_path.parent() else {
            return;
        };
        for request in &mut self.collection.requests {
            if let Some(path) = &mut request.body.file_path {
                if path.is_relative() && !path.to_string_lossy().contains("{{") {
                    *path = base.join(&path);
                }
            }
            for field in &mut request.body.form {
                let path = Path::new(&field.value);
                if field.file && path.is_relative() && !field.value.contains("{{") {
                    field.value = base.join(path).display().to_string();
                }
            }
        }
    }

    fn walk(&mut self, items: &[Value], prefix: &str, inherited_auth: Option<&Value>) {
        for item in items {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Untitled");
            let full_name = if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{prefix} / {name}")
            };
            let auth = item.get("auth").or(inherited_auth);
            if let Some(children) = item.get("item").and_then(Value::as_array) {
                if !self.collection.folders.contains(&full_name) {
                    self.collection.folders.push(full_name.clone());
                }
                self.walk(children, &full_name, auth);
            } else if let Some(req) = item.get("request") {
                let auth = req.get("auth").or(auth);
                if auth
                    .and_then(|a| a.get("type"))
                    .and_then(Value::as_str)
                    .is_some_and(|kind| {
                        !matches!(kind, "noauth" | "bearer" | "basic" | "apikey" | "oauth2")
                    })
                {
                    self.unsupported_auth += 1;
                }
                if let Some(mut parsed) =
                    parse_request(req, name, auth, &mut self.unsupported_bodies)
                {
                    parsed.scripts = parse_scripts(item.get("event"));
                    if let Some(behavior) = item.get("protocolProfileBehavior") {
                        parsed.follow_redirects =
                            behavior.get("followRedirects").and_then(Value::as_bool);
                        parsed.max_redirects = behavior
                            .get("maxRedirects")
                            .and_then(Value::as_u64)
                            .map(|n| n as u32);
                    }
                    parsed.collection_id = Some(self.collection.id.clone());
                    parsed.folder = (!prefix.is_empty()).then(|| prefix.to_string());
                    self.collection.requests.push(parsed);
                } else {
                    self.skipped += 1;
                }
            }
        }
    }
}

pub fn is_postman(doc: &Value) -> bool {
    let Some(schema) = doc
        .get("info")
        .and_then(|i| i.get("schema"))
        .and_then(Value::as_str)
    else {
        return false;
    };
    doc.get("item").is_some_and(Value::is_array)
        && (schema.contains("collection/v2.0.0/") || schema.contains("collection/v2.1.0/"))
}

fn string(v: &Value) -> String {
    v.as_str().map(str::to_string).unwrap_or_else(|| {
        if v.is_null() {
            String::new()
        } else {
            v.to_string()
        }
    })
}

fn description(v: &Value) -> String {
    v.as_str()
        .or_else(|| v.get("content").and_then(Value::as_str))
        .unwrap_or("")
        .to_string()
}

fn key_values(value: Option<&Value>, key_field: &str) -> Vec<KeyValue> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| {
            let key = v.get(key_field)?.as_str()?;
            Some(
                KeyValue::new(key, v.get("value").map(string).unwrap_or_default())
                    .description(v.get("description").map(description).unwrap_or_default())
                    .enabled(!v.get("disabled").and_then(Value::as_bool).unwrap_or(false)),
            )
        })
        .collect()
}

/// `item.event[]`: `prerequest` / `test` scripts (`script.exec` is an array of
/// lines or a single string). Several events of one kind are joined.
fn parse_scripts(events: Option<&Value>) -> Scripts {
    let mut scripts = Scripts::default();
    for event in events.and_then(Value::as_array).into_iter().flatten() {
        if event.get("disabled").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let target = match event.get("listen").and_then(Value::as_str) {
            Some("prerequest") => &mut scripts.pre_request,
            Some("test") => &mut scripts.post_response,
            _ => continue,
        };
        let code = match event.pointer("/script/exec") {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Array(lines)) => lines
                .iter()
                .map(|line| line.as_str().unwrap_or_default())
                .collect::<Vec<_>>()
                .join("\n"),
            _ => continue,
        };
        if code.trim().is_empty() {
            continue;
        }
        if !target.is_empty() {
            target.push('\n');
        }
        target.push_str(&code);
    }
    scripts
}

fn parse_request(
    req: &Value,
    name: &str,
    auth: Option<&Value>,
    unsupported_bodies: &mut usize,
) -> Option<RequestSpec> {
    let method = match req.get("method").and_then(Value::as_str) {
        Some(name) => Method::parse(name)?,
        None => Method::Get,
    };
    let url = req.as_str().map(|_| req).or_else(|| req.get("url"))?;
    let raw = url
        .as_str()
        .or_else(|| url.get("raw").and_then(Value::as_str))
        .map(str::to_string)
        .or_else(|| url_from_parts(url))?;
    let structured_query = key_values(url.get("query"), "key");
    let mut raw = if structured_query.is_empty() {
        raw
    } else {
        raw.split_once('?')
            .map_or(raw.clone(), |(base, _)| base.to_string())
    };
    for variable in key_values(url.get("variable"), "key") {
        let replacement = if variable.value.is_empty() {
            format!("{{{{{}}}}}", variable.key)
        } else {
            variable.value
        };
        raw = raw.replace(&format!(":{}", variable.key), &replacement);
    }
    let mut request = RequestSpec::new(name, method, raw);
    request.params = structured_query;
    request.headers = key_values(req.get("header"), "key");
    request.auth = parse_auth(auth);
    if let Some(body) = req.get("body") {
        match body.get("mode").and_then(Value::as_str) {
            Some("raw") => {
                let text = body
                    .get("raw")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let language = body
                    .pointer("/options/raw/language")
                    .and_then(Value::as_str);
                let json_header = request.headers.iter().any(|h| {
                    h.key.eq_ignore_ascii_case("content-type")
                        && h.value.to_ascii_lowercase().contains("json")
                });
                request.body = Body {
                    kind: if language == Some("json") || json_header {
                        BodyKind::Json
                    } else {
                        BodyKind::Text
                    },
                    text,
                    form: Vec::new(),
                    file_path: None,
                };
            }
            Some("urlencoded") => {
                request.body = Body {
                    kind: BodyKind::Form,
                    text: String::new(),
                    form: key_values(body.get("urlencoded"), "key"),
                    file_path: None,
                }
            }
            Some("formdata") => {
                let form = body
                    .get("formdata")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|entry| {
                        let key = entry.get("key")?.as_str()?;
                        let file = entry.get("type").and_then(Value::as_str) == Some("file");
                        let value = if file {
                            entry
                                .get("src")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string()
                        } else {
                            entry.get("value").map(string).unwrap_or_default()
                        };
                        let mut row = KeyValue::new(key, value)
                            .description(entry.get("description").map(description).unwrap_or_default())
                            .enabled(
                                !entry
                                    .get("disabled")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(false),
                            );
                        row.file = file;
                        Some(row)
                    })
                    .collect();
                request.body = Body {
                    kind: BodyKind::Multipart,
                    form,
                    ..Body::default()
                };
            }
            Some("file") => {
                let path = body
                    .pointer("/file/src")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                request.body = Body {
                    kind: BodyKind::Binary,
                    file_path: (!path.is_empty()).then(|| path.into()),
                    ..Body::default()
                };
            }
            Some("graphql") => {
                let query = body
                    .pointer("/graphql/query")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let variables = body
                    .pointer("/graphql/variables")
                    .and_then(Value::as_str)
                    .unwrap_or("{}");
                request.body = Body::json(serde_json::json!({"query": query, "variables": serde_json::from_str::<Value>(variables).unwrap_or(Value::Object(Default::default()))}).to_string());
            }
            Some(_) => *unsupported_bodies += 1,
            _ => {}
        }
    }
    Some(request)
}

fn url_from_parts(url: &Value) -> Option<String> {
    let host = url.get("host")?;
    let join = |v: &Value| -> Option<String> {
        v.as_str().map(str::to_string).or_else(|| {
            v.as_array().map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(".")
            })
        })
    };
    let host = join(host)?;
    let protocol = url
        .get("protocol")
        .and_then(Value::as_str)
        .unwrap_or("https");
    let path = url
        .get("path")
        .and_then(|p| {
            p.as_str().map(str::to_string).or_else(|| {
                p.as_array().map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("/")
                })
            })
        })
        .unwrap_or_default();
    Some(format!(
        "{protocol}://{host}/{}",
        path.trim_start_matches('/')
    ))
}

fn parse_auth(auth: Option<&Value>) -> Auth {
    let Some(auth) = auth else { return Auth::None };
    let field = |kind: &str, key: &str| -> String {
        auth.get(kind)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|entry| entry.get("key").and_then(Value::as_str) == Some(key))
            .and_then(|entry| entry.get("value"))
            .map(string)
            .unwrap_or_default()
    };
    match auth.get("type").and_then(Value::as_str) {
        Some("bearer") => Auth::Bearer {
            token: field("bearer", "token"),
        },
        Some("oauth2") => Auth::Bearer {
            token: {
                let access = field("oauth2", "accessToken");
                if access.is_empty() {
                    field("oauth2", "token")
                } else {
                    access
                }
            },
        },
        Some("basic") => Auth::Basic {
            username: field("basic", "username"),
            password: field("basic", "password"),
        },
        Some("apikey") => Auth::ApiKey {
            key: field("apikey", "key"),
            value: field("apikey", "value"),
            in_query: field("apikey", "in").eq_ignore_ascii_case("query"),
        },
        _ => Auth::None,
    }
}
