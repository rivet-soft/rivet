//! Mock servers for collections, run in Docker. The engine (for now only
//! WireMock) gets stub mappings generated from the collection's saved
//! examples: every example answers its request's method and path; a specific
//! example is picked with `x-mock-response-name` / `x-mock-response-code`
//! headers, as with Postman mocks.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::export::split_url;
use super::types::Collection;

/// Mock engine (Settings → Моки). Only WireMock for now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MockEngine {
    #[default]
    #[serde(rename = "wiremock")]
    WireMock,
}

impl MockEngine {
    pub const ALL: [MockEngine; 1] = [MockEngine::WireMock];

    pub fn label(self) -> &'static str {
        match self {
            Self::WireMock => "WireMock",
        }
    }

    pub fn default_image(self) -> &'static str {
        match self {
            Self::WireMock => "wiremock/wiremock:3.9.1",
        }
    }

    /// Port the engine listens on inside its container.
    pub fn container_port(self) -> u16 {
        match self {
            Self::WireMock => 8080,
        }
    }

    /// Where the engine reads its files inside the container.
    pub fn container_root(self) -> &'static str {
        match self {
            Self::WireMock => "/home/wiremock",
        }
    }

    /// GET path that answers 200 once the server is up.
    pub fn health_path(self) -> &'static str {
        match self {
            Self::WireMock => "/__admin/health",
        }
    }

    /// POST path that reloads stubs from the mounted files.
    pub fn reload_path(self) -> &'static str {
        match self {
            Self::WireMock => "/__admin/mappings/reset",
        }
    }
}

/// Settings → Моки.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockSettings {
    #[serde(default)]
    pub engine: MockEngine,
    /// First host port to try; the next free one is used when it's taken.
    #[serde(default = "default_port")]
    pub port: u16,
    /// Docker image; empty = the engine's default.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub image: String,
}

fn default_port() -> u16 {
    8089
}

impl Default for MockSettings {
    fn default() -> Self {
        Self { engine: MockEngine::default(), port: default_port(), image: String::new() }
    }
}

impl MockSettings {
    pub fn image(&self) -> &str {
        if self.image.trim().is_empty() { self.engine.default_image() } else { self.image.trim() }
    }
}

// ---- WireMock mappings ---------------------------------------------------------

/// Response headers not worth replaying (the stub body is plain text).
fn replayed_header(name: &str) -> bool {
    !matches!(
        name.to_ascii_lowercase().as_str(),
        "content-length" | "transfer-encoding" | "connection" | "content-encoding" | "date" | "keep-alive"
    )
}

/// Regex-escapes a literal path segment.
fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// WireMock URL matcher for a request URL: exact `urlPath`, or a
/// `urlPathPattern` when the path has `{{var}}` / `:var` segments.
fn url_matcher(url: &str) -> (&'static str, String) {
    let (_, path, vars) = split_url(url);
    if vars.is_empty() {
        return ("urlPath", path);
    }
    let pattern = path
        .split('/')
        .map(|seg| {
            if seg.starts_with('{') && seg.ends_with('}') {
                "[^/]+".to_string()
            } else {
                regex_escape(seg)
            }
        })
        .collect::<Vec<_>>()
        .join("/");
    ("urlPathPattern", pattern)
}

/// `{"mappings": [...]}` for WireMock from the collection's examples, plus
/// notes (requests without examples get no stub).
pub fn wiremock_mappings(c: &Collection) -> (Value, Vec<String>) {
    let mut mappings = Vec::new();
    let mut without = Vec::new();
    for r in c.requests.iter().filter(|r| !r.is_ws()) {
        if r.examples.is_empty() {
            without.push(r.name.clone());
            continue;
        }
        let (match_kind, url) = url_matcher(&r.url);
        let method = r.method.full_label();
        for (i, e) in r.examples.iter().enumerate() {
            let mut headers = Map::new();
            for h in e.headers.iter().filter(|h| replayed_header(&h.key)) {
                headers.insert(h.key.clone(), json!(h.value));
            }
            let response = json!({"status": e.status, "headers": headers, "body": e.body});
            let stub = |priority: u32, extra: Option<(&str, String)>| {
                let mut request = json!({"method": method, match_kind: url});
                if let Some((header, value)) = extra {
                    request["headers"] = json!({header: {"equalTo": value}});
                }
                json!({
                    "name": format!("{} · {}", r.name, e.name),
                    "priority": priority,
                    "request": request,
                    "response": response,
                    "metadata": {"rivet": {"request_id": r.id, "example_id": e.id}},
                })
            };
            mappings.push(stub(1, Some(("x-mock-response-name", e.name.clone()))));
            mappings.push(stub(2, Some(("x-mock-response-code", e.status.to_string()))));
            // The first example answers when no header picks one.
            if i == 0 {
                mappings.push(stub(5, None));
            }
        }
    }
    let mut notes = Vec::new();
    if !without.is_empty() {
        notes.push(format!(
            "без примеров (мок их не знает): {} — сохраните ответ закладкой в ответе",
            without.join(", ")
        ));
    }
    (json!({"mappings": mappings}), notes)
}

/// Number of example-backed requests the mock answers.
pub fn answered_requests(c: &Collection) -> usize {
    c.requests.iter().filter(|r| !r.is_ws() && !r.examples.is_empty()).count()
}

// ---- Docker --------------------------------------------------------------------

/// Docker CLI location. GUI apps on macOS don't inherit the terminal's PATH,
/// so the usual install locations are checked too.
pub fn find_docker() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "docker.exe" } else { "docker" };
    let from_path = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|dir| dir.join(exe));
    let home = dirs::home_dir().unwrap_or_default();
    let known: Vec<PathBuf> = if cfg!(windows) {
        vec![
            PathBuf::from(r"C:\Program Files\Docker\Docker\resources\bin\docker.exe"),
        ]
    } else {
        vec![
            PathBuf::from("/usr/local/bin/docker"),
            PathBuf::from("/opt/homebrew/bin/docker"),
            PathBuf::from("/Applications/Docker.app/Contents/Resources/bin/docker"),
            home.join(".docker/bin/docker"),
            home.join(".orbstack/bin/docker"),
            home.join(".rd/bin/docker"),
            PathBuf::from("/usr/bin/docker"),
        ]
    };
    from_path.chain(known).find(|p| p.is_file())
}

/// Container name for a collection's mock.
pub fn container_name(collection_id: &str) -> String {
    let short: String = collection_id.chars().filter(|c| c.is_ascii_alphanumeric()).take(12).collect();
    format!("rivet-mock-{short}")
}

/// `docker run …` arguments for a mock serving `dir` on host `port`.
pub fn run_args(settings: &MockSettings, name: &str, dir: &Path, port: u16) -> Vec<String> {
    let engine = settings.engine;
    vec![
        "run".into(),
        "-d".into(),
        "--rm".into(),
        "--name".into(),
        name.into(),
        "-p".into(),
        format!("127.0.0.1:{port}:{}", engine.container_port()),
        "-v".into(),
        format!("{}:{}", dir.display(), engine.container_root()),
        settings.image().into(),
        "--disable-banner".into(),
    ]
}

/// Writes the engine's files for `c` into `dir` (WireMock: `mappings/rivet.json`).
/// Returns the notes from mapping generation.
pub fn write_files(engine: MockEngine, c: &Collection, dir: &Path) -> Result<Vec<String>, String> {
    match engine {
        MockEngine::WireMock => {
            let (mappings, notes) = wiremock_mappings(c);
            let mappings_dir = dir.join("mappings");
            std::fs::create_dir_all(&mappings_dir).map_err(|e| format!("{}: {e}", mappings_dir.display()))?;
            std::fs::create_dir_all(dir.join("__files")).ok();
            let text = serde_json::to_string_pretty(&mappings).map_err(|e| e.to_string())?;
            let file = mappings_dir.join("rivet.json");
            std::fs::write(&file, text).map_err(|e| format!("{}: {e}", file.display()))?;
            Ok(notes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{Example, KeyValue, Method, RequestSpec};

    fn example(name: &str, status: u16, body: &str) -> Example {
        Example {
            id: format!("e-{name}"),
            name: name.into(),
            status,
            status_text: String::new(),
            headers: vec![KeyValue::new("Content-Type", "application/json"), KeyValue::new("Content-Length", "9")],
            body: body.into(),
            url: None,
            saved_at: 0,
        }
    }

    #[test]
    fn mappings_from_examples() {
        let mut c = Collection::new("Shop");
        let mut list = RequestSpec::new("List", Method::Get, "{{baseUrl}}/users");
        list.examples = vec![example("ok", 200, "[1]"), example("empty", 200, "[]")];
        let mut one = RequestSpec::new("One", Method::Get, "https://api.x/users/:id/v1.0");
        one.examples = vec![example("missing", 404, "{}")];
        let bare = RequestSpec::new("Bare", Method::Post, "{{baseUrl}}/orders");
        c.requests = vec![list, one, bare, RequestSpec::untitled_ws()];
        let (doc, notes) = wiremock_mappings(&c);
        let m = doc["mappings"].as_array().unwrap();
        // List: name + code stubs for both examples, default for the first.
        assert_eq!(m.len(), 2 + 2 + 1 + 2 + 1);
        assert_eq!(m[0]["request"], json!({"method": "GET", "urlPath": "/users", "headers": {"x-mock-response-name": {"equalTo": "ok"}}}));
        assert_eq!(m[0]["priority"], 1);
        assert_eq!(m[1]["request"]["headers"], json!({"x-mock-response-code": {"equalTo": "200"}}));
        assert_eq!(m[2]["priority"], 5);
        assert!(m[2]["request"].get("headers").is_none(), "default stub");
        assert_eq!(m[2]["response"], json!({"status": 200, "headers": {"Content-Type": "application/json"}, "body": "[1]"}));
        let one = m.iter().find(|s| s["name"] == "One · missing" && s["priority"] == 5).unwrap();
        assert_eq!(one["request"]["urlPathPattern"], "/users/[^/]+/v1\\.0");
        assert_eq!(one["response"]["status"], 404);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("Bare"));
        assert_eq!(answered_requests(&c), 2);
    }

    #[test]
    fn docker_arguments() {
        let s = MockSettings::default();
        assert_eq!(s.image(), "wiremock/wiremock:3.9.1");
        let args = run_args(&s, "rivet-mock-abc", Path::new("/data/mocks/abc"), 8090);
        assert_eq!(
            args,
            [
                "run", "-d", "--rm", "--name", "rivet-mock-abc", "-p", "127.0.0.1:8090:8080", "-v",
                "/data/mocks/abc:/home/wiremock", "wiremock/wiremock:3.9.1", "--disable-banner"
            ]
        );
        let custom = MockSettings { image: " wiremock/wiremock:latest ".into(), ..MockSettings::default() };
        assert_eq!(custom.image(), "wiremock/wiremock:latest");
        assert_eq!(container_name("1f2e-3d4c-5b6a-7980-xyz"), "rivet-mock-1f2e3d4c5b6a");
        let parsed: MockSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed, MockSettings::default());
        assert_eq!(serde_json::to_value(MockEngine::WireMock).unwrap(), "wiremock");
    }

    #[test]
    fn files_are_written() {
        let dir = std::env::temp_dir().join(format!("rivet-mock-{}", crate::core::types::new_id()));
        let mut c = Collection::new("S");
        let mut r = RequestSpec::new("A", Method::Get, "http://h/a");
        r.examples = vec![example("ok", 200, "hi")];
        c.requests.push(r);
        write_files(MockEngine::WireMock, &c, &dir).unwrap();
        let text = std::fs::read_to_string(dir.join("mappings/rivet.json")).unwrap();
        assert!(text.contains("\"urlPath\": \"/a\""));
        std::fs::remove_dir_all(&dir).ok();
    }
}
