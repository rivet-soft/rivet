//! Mock servers for collections, run in Docker. The engine (for now only
//! WireMock) gets stub mappings generated from the collection's saved
//! examples: every example answers its request's method and path; a specific
//! example is picked with `x-mock-response-name` / `x-mock-response-code`
//! headers, as with Postman mocks.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::export::split_url;
use super::openapi::Spec;
use super::types::{Collection, RequestSpec};

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

/// How a mock answers a collection's requests.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MockCoverage {
    /// Requests answered with their saved examples.
    pub from_examples: usize,
    /// Requests without examples answered from the linked OpenAPI spec.
    pub from_spec: usize,
    /// Requests with neither: they get a 501 that says so.
    pub missing: Vec<String>,
}

/// A stub matching `method` + the request's URL; `pick` adds a header match.
fn stub(
    r: &RequestSpec,
    name: String,
    priority: u32,
    pick: Option<(&str, String)>,
    response: Value,
    example_id: Option<&str>,
) -> Value {
    let (match_kind, url) = url_matcher(&r.url);
    let mut request = json!({"method": r.method.full_label(), match_kind: url});
    if let Some((header, value)) = pick {
        request["headers"] = json!({header: {"equalTo": value}});
    }
    json!({
        "name": name,
        "priority": priority,
        "request": request,
        "response": response,
        "metadata": {"rivet": {"request_id": r.id, "example_id": example_id}},
    })
}

/// `{"mappings": [...]}` for WireMock. Each request is answered by its saved
/// examples; without any, by the responses its linked OpenAPI operation
/// declares (examples or schema-generated); otherwise by a 501 explaining
/// that it has no example.
pub fn wiremock_mappings(c: &Collection, spec: Option<&Spec>) -> (Value, MockCoverage) {
    let mut mappings = Vec::new();
    let mut coverage = MockCoverage::default();
    for r in c.requests.iter().filter(|r| !r.is_ws()) {
        if !r.examples.is_empty() {
            coverage.from_examples += 1;
            for (i, e) in r.examples.iter().enumerate() {
                let mut headers = Map::new();
                for h in e.headers.iter().filter(|h| replayed_header(&h.key)) {
                    headers.insert(h.key.clone(), json!(h.value));
                }
                let response = json!({"status": e.status, "headers": headers, "body": e.body});
                let name = format!("{} · {}", r.name, e.name);
                let id = Some(e.id.as_str());
                mappings.push(stub(r, name.clone(), 1, Some(("x-mock-response-name", e.name.clone())), response.clone(), id));
                mappings.push(stub(r, name.clone(), 2, Some(("x-mock-response-code", e.status.to_string())), response.clone(), id));
                // The first example answers when no header picks one.
                if i == 0 {
                    mappings.push(stub(r, name, 5, None, response, id));
                }
            }
            continue;
        }
        let declared = match (spec, &r.spec_ref) {
            (Some(spec), Some(spec_ref)) => spec.response_examples(spec_ref),
            _ => Vec::new(),
        };
        if !declared.is_empty() {
            coverage.from_spec += 1;
            for (i, (status, body)) in declared.into_iter().enumerate() {
                let headers = if body.is_empty() { json!({}) } else { json!({"Content-Type": "application/json"}) };
                let response = json!({"status": status, "headers": headers, "body": body});
                let name = format!("{} · спецификация {status}", r.name);
                mappings.push(stub(r, name.clone(), 3, Some(("x-mock-response-code", status.to_string())), response.clone(), None));
                if i == 0 {
                    mappings.push(stub(r, name, 6, None, response, None));
                }
            }
            continue;
        }
        coverage.missing.push(r.name.clone());
        let body = json!({
            "error": "У этого запроса нет сохранённого примера",
            "request": r.name,
            "hint": "Отправьте запрос в Rivet и сохраните ответ закладкой в заголовке ответа, затем «Обновить» мок",
        });
        let response = json!({
            "status": 501,
            "headers": {"Content-Type": "application/json; charset=utf-8"},
            "body": body.to_string(),
        });
        mappings.push(stub(r, format!("{} · нет примера", r.name), 9, None, response, None));
    }
    (json!({"mappings": mappings}), coverage)
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
/// Labels let the app and `rivet mcp` find each other's mocks.
const LABEL_COLLECTION: &str = "rivet.collection";
const LABEL_PORT: &str = "rivet.port";
const LABEL_ENGINE: &str = "rivet.engine";
const LABEL_DIR: &str = "rivet.dir";

/// Folder with a collection's mock files: `mocks/<collection id>` next to
/// project.json (shared by the window and `rivet mcp`).
pub fn mock_dir(project_path: Option<&Path>, collection_id: &str) -> PathBuf {
    project_path
        .and_then(|p| p.parent().map(|d| d.join("mocks")))
        .unwrap_or_else(|| std::env::temp_dir().join("rivet-mocks"))
        .join(collection_id)
}

pub fn run_args(settings: &MockSettings, collection_id: &str, name: &str, dir: &Path, port: u16) -> Vec<String> {
    let engine = settings.engine;
    let engine_id = serde_json::to_value(engine).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    vec![
        "run".into(),
        "-d".into(),
        "--rm".into(),
        "--name".into(),
        name.into(),
        "--label".into(),
        format!("{LABEL_COLLECTION}={collection_id}"),
        "--label".into(),
        format!("{LABEL_PORT}={port}"),
        "--label".into(),
        format!("{LABEL_ENGINE}={engine_id}"),
        "--label".into(),
        format!("{LABEL_DIR}={}", dir.display()),
        "-p".into(),
        format!("127.0.0.1:{port}:{}", engine.container_port()),
        "-v".into(),
        format!("{}:{}", dir.display(), engine.container_root()),
        settings.image().into(),
        "--disable-banner".into(),
    ]
}

/// Writes the engine's files for `c` into `dir` (WireMock: `mappings/rivet.json`).
/// Returns how the requests are covered.
pub fn write_files(engine: MockEngine, c: &Collection, spec: Option<&Spec>, dir: &Path) -> Result<MockCoverage, String> {
    match engine {
        MockEngine::WireMock => {
            let (mappings, coverage) = wiremock_mappings(c, spec);
            let mappings_dir = dir.join("mappings");
            std::fs::create_dir_all(&mappings_dir).map_err(|e| format!("{}: {e}", mappings_dir.display()))?;
            std::fs::create_dir_all(dir.join("__files")).ok();
            let text = serde_json::to_string_pretty(&mappings).map_err(|e| e.to_string())?;
            let file = mappings_dir.join("rivet.json");
            std::fs::write(&file, text).map_err(|e| format!("{}: {e}", file.display()))?;
            Ok(coverage)
        }
    }
}

// ---- running -------------------------------------------------------------------

/// Whether mocks can run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DockerStatus {
    /// No Docker CLI found.
    Missing,
    /// The CLI is there but the daemon doesn't answer (Docker Desktop not started).
    NotRunning { docker: PathBuf, message: String },
    Ready { docker: PathBuf, version: String },
}

impl DockerStatus {
    pub fn docker(&self) -> Option<&Path> {
        match self {
            Self::Ready { docker, .. } => Some(docker),
            _ => None,
        }
    }
}

/// Output of a finished command.
struct Ran {
    ok: bool,
    stdout: String,
    stderr: String,
}

/// Runs a command, giving up (and killing it) after `timeout`.
fn run(program: &Path, args: &[String], timeout: std::time::Duration) -> Result<Ran, String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: {e}", program.display()))?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            let mut stdout = String::new();
            let mut stderr = String::new();
            child.stdout.take().map(|mut o| o.read_to_string(&mut stdout));
            child.stderr.take().map(|mut e| e.read_to_string(&mut stderr));
            return Ok(Ran { ok: status.success(), stdout, stderr });
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{} не ответил за {} с", program.display(), timeout.as_secs()));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Finds Docker and asks the daemon for its version.
pub fn docker_status() -> DockerStatus {
    let Some(docker) = find_docker() else { return DockerStatus::Missing };
    let args = ["version".to_string(), "--format".into(), "{{.Server.Version}}".into()];
    match run(&docker, &args, std::time::Duration::from_secs(8)) {
        Ok(r) if r.ok && !r.stdout.trim().is_empty() => {
            DockerStatus::Ready { docker, version: r.stdout.trim().to_string() }
        }
        Ok(r) => DockerStatus::NotRunning {
            docker,
            message: first_line(&r.stderr).unwrap_or("Docker не отвечает").to_string(),
        },
        Err(message) => DockerStatus::NotRunning { docker, message },
    }
}

fn first_line(s: &str) -> Option<&str> {
    s.lines().map(str::trim).find(|l| !l.is_empty())
}

/// First port from `from` that is not in `taken` and can be bound on 127.0.0.1.
pub fn free_port(from: u16, taken: &[u16]) -> Option<u16> {
    (from..=u16::MAX)
        .take(200)
        .find(|p| !taken.contains(p) && std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok())
}

/// A running mock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockRun {
    pub engine: MockEngine,
    pub container: String,
    pub port: u16,
    /// `http://127.0.0.1:<port>`.
    pub url: String,
    pub dir: PathBuf,
    pub coverage: MockCoverage,
}

fn http(method: super::types::Method, url: String, timeout_ms: u64) -> Result<u16, String> {
    let req = super::types::ResolvedRequest { method, url, headers: Vec::new(), body: None, missing_vars: Vec::new(), omit_default_headers: Vec::new() };
    let settings = super::types::Settings { timeout_ms, follow_redirects: false, ..Default::default() };
    super::http::execute(&req, &settings, &super::http::CancelToken::new())
        .map(|r| r.status)
        .map_err(|e| e.to_string())
}

/// Polls the engine's health endpoint until it answers 200.
fn wait_healthy(engine: MockEngine, url: &str, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    let health = format!("{url}{}", engine.health_path());
    while std::time::Instant::now() < deadline {
        if http(super::types::Method::Get, health.clone(), 1500) == Ok(200) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    false
}

/// Starts collection `c`'s mock in Docker on `port`, serving files from `dir`.
/// Blocking (the first start may pull the image): call off the UI thread.
pub fn start(
    docker: &Path,
    settings: &MockSettings,
    c: &Collection,
    spec: Option<&Spec>,
    dir: &Path,
    port: u16,
) -> Result<MockRun, String> {
    let engine = settings.engine;
    let coverage = write_files(engine, c, spec, dir)?;
    let name = container_name(&c.id);
    // A container left over from a crash would hold the name.
    let _ = run(docker, &["rm".into(), "-f".into(), name.clone()], std::time::Duration::from_secs(20));
    let ran = run(docker, &run_args(settings, &c.id, &name, dir, port), std::time::Duration::from_secs(600))?;
    if !ran.ok {
        let reason = first_line(&ran.stderr).unwrap_or("неизвестная ошибка");
        return Err(format!("Docker не запустил мок: {reason}"));
    }
    let url = format!("http://127.0.0.1:{port}");
    if !wait_healthy(engine, &url, std::time::Duration::from_secs(60)) {
        let _ = run(docker, &["stop".into(), "-t".into(), "1".into(), name], std::time::Duration::from_secs(20));
        return Err(format!("{} не ответил на {url} за минуту", engine.label()));
    }
    Ok(MockRun { engine, container: name, port, url, dir: dir.to_path_buf(), coverage })
}

/// Mocks running in Docker (started by the window or by `rivet mcp`), by
/// collection id. Coverage is unknown here (empty).
pub fn running_mocks(docker: &Path) -> Result<Vec<(String, MockRun)>, String> {
    let format = format!(
        "{{{{.Names}}}}\t{{{{.Label \"{LABEL_COLLECTION}\"}}}}\t{{{{.Label \"{LABEL_PORT}\"}}}}\t{{{{.Label \"{LABEL_ENGINE}\"}}}}\t{{{{.Label \"{LABEL_DIR}\"}}}}"
    );
    let args = ["ps".to_string(), "--filter".into(), format!("label={LABEL_COLLECTION}"), "--format".into(), format];
    let ran = run(docker, &args, std::time::Duration::from_secs(15))?;
    if !ran.ok {
        return Err(first_line(&ran.stderr).unwrap_or("docker ps не сработал").to_string());
    }
    Ok(ran
        .stdout
        .lines()
        .filter_map(|line| {
            let mut f = line.split('\t');
            let (name, id, port, engine, dir) = (f.next()?, f.next()?, f.next()?, f.next()?, f.next()?);
            let port: u16 = port.parse().ok()?;
            let engine: MockEngine = serde_json::from_value(json!(engine)).unwrap_or_default();
            Some((
                id.to_string(),
                MockRun {
                    engine,
                    container: name.to_string(),
                    port,
                    url: format!("http://127.0.0.1:{port}"),
                    dir: PathBuf::from(dir),
                    coverage: MockCoverage::default(),
                },
            ))
        })
        .collect())
}

/// Rewrites the stubs of a running mock from `c` and reloads them (no restart).
pub fn reload(running: &MockRun, c: &Collection, spec: Option<&Spec>) -> Result<MockRun, String> {
    let coverage = write_files(running.engine, c, spec, &running.dir)?;
    let status = http(super::types::Method::Post, format!("{}{}", running.url, running.engine.reload_path()), 5000)?;
    if !(200..300).contains(&status) {
        return Err(format!("{} ответил {status} на перезагрузку заглушек", running.engine.label()));
    }
    Ok(MockRun { coverage, ..running.clone() })
}

/// Stops a mock (its container is removed: it was started with --rm).
pub fn stop(docker: &Path, running: &MockRun) -> Result<(), String> {
    let ran = run(docker, &["stop".into(), "-t".into(), "1".into(), running.container.clone()], std::time::Duration::from_secs(30))?;
    if ran.ok || ran.stderr.contains("No such container") {
        Ok(())
    } else {
        Err(first_line(&ran.stderr).unwrap_or("docker stop не сработал").to_string())
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
        let (doc, coverage) = wiremock_mappings(&c, None);
        let m = doc["mappings"].as_array().unwrap();
        // List: name + code stubs for both examples, default for the first;
        // One: 3; Bare: a 501 stub.
        assert_eq!(m.len(), 2 + 2 + 1 + 2 + 1 + 1);
        assert_eq!(m[0]["request"], json!({"method": "GET", "urlPath": "/users", "headers": {"x-mock-response-name": {"equalTo": "ok"}}}));
        assert_eq!(m[0]["priority"], 1);
        assert_eq!(m[1]["request"]["headers"], json!({"x-mock-response-code": {"equalTo": "200"}}));
        assert_eq!(m[2]["priority"], 5);
        assert!(m[2]["request"].get("headers").is_none(), "default stub");
        assert_eq!(m[2]["response"], json!({"status": 200, "headers": {"Content-Type": "application/json"}, "body": "[1]"}));
        let one = m.iter().find(|s| s["name"] == "One · missing" && s["priority"] == 5).unwrap();
        assert_eq!(one["request"]["urlPathPattern"], "/users/[^/]+/v1\\.0");
        assert_eq!(one["response"]["status"], 404);
        assert_eq!(coverage, MockCoverage { from_examples: 2, from_spec: 0, missing: vec!["Bare".into()] });
        let bare = m.iter().find(|s| s["name"] == "Bare · нет примера").unwrap();
        assert_eq!(bare["response"]["status"], 501);
        assert_eq!(bare["priority"], 9);
        assert!(bare["response"]["body"].as_str().unwrap().contains("нет сохранённого примера"));
        assert_eq!(answered_requests(&c), 2);
    }

    #[test]
    fn requests_without_examples_answer_from_the_spec() {
        let spec = Spec::parse(
            "openapi: 3.0.3\ninfo: {title: T, version: '1'}\npaths:\n  /orders/{id}:\n    get:\n      responses:\n        '404': {description: no}\n        '200':\n          description: ok\n          content:\n            application/json:\n              example: {id: 7, state: paid}\n        default: {description: err}\n",
        )
        .unwrap();
        let mut c = Collection::new("Spec");
        let mut r = RequestSpec::new("Order", Method::Get, "{{baseUrl}}/orders/{{id}}");
        r.spec_ref = Some(crate::core::types::SpecRef { method: Method::Get, path: "/orders/{id}".into() });
        c.requests.push(r);
        let (doc, coverage) = wiremock_mappings(&c, Some(&spec));
        assert_eq!(coverage, MockCoverage { from_examples: 0, from_spec: 1, missing: vec![] });
        let m = doc["mappings"].as_array().unwrap();
        // 200 first (success), then 404; `default` skipped; default stub = 200.
        let default = m.iter().find(|s| s["priority"] == 6).unwrap();
        assert_eq!(default["response"]["status"], 200);
        let body: Value = serde_json::from_str(default["response"]["body"].as_str().unwrap()).unwrap();
        assert_eq!(body, json!({"id": 7, "state": "paid"}));
        assert_eq!(default["request"]["urlPathPattern"], "/orders/[^/]+");
        assert!(m.iter().any(|s| s["request"]["headers"]["x-mock-response-code"]["equalTo"] == "404"));
        assert_eq!(m.len(), 3);
        // Without the spec the same request has no answer.
        assert_eq!(wiremock_mappings(&c, None).1.missing, vec!["Order".to_string()]);
    }

    #[test]
    fn docker_arguments() {
        let s = MockSettings::default();
        assert_eq!(s.image(), "wiremock/wiremock:3.9.1");
        let args = run_args(&s, "c1", "rivet-mock-abc", Path::new("/data/mocks/abc"), 8090);
        assert_eq!(
            args,
            [
                "run", "-d", "--rm", "--name", "rivet-mock-abc",
                "--label", "rivet.collection=c1", "--label", "rivet.port=8090",
                "--label", "rivet.engine=wiremock", "--label", "rivet.dir=/data/mocks/abc",
                "-p", "127.0.0.1:8090:8080", "-v",
                "/data/mocks/abc:/home/wiremock", "wiremock/wiremock:3.9.1", "--disable-banner"
            ]
        );
        assert_eq!(mock_dir(Some(Path::new("/p/project.json")), "c1"), Path::new("/p/mocks/c1"));
        let custom = MockSettings { image: " wiremock/wiremock:latest ".into(), ..MockSettings::default() };
        assert_eq!(custom.image(), "wiremock/wiremock:latest");
        assert_eq!(container_name("1f2e-3d4c-5b6a-7980-xyz"), "rivet-mock-1f2e3d4c5b6a");
        let parsed: MockSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed, MockSettings::default());
        assert_eq!(serde_json::to_value(MockEngine::WireMock).unwrap(), "wiremock");
    }

    #[test]
    fn free_ports_skip_taken_and_bound() {
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let busy = held.local_addr().unwrap().port();
        let port = free_port(busy, &[busy + 1]).unwrap();
        assert!(port != busy && port != busy + 1, "{busy} → {port}");
    }

    #[test]
    fn files_are_written() {
        let dir = std::env::temp_dir().join(format!("rivet-mock-{}", crate::core::types::new_id()));
        let mut c = Collection::new("S");
        let mut r = RequestSpec::new("A", Method::Get, "http://h/a");
        r.examples = vec![example("ok", 200, "hi")];
        c.requests.push(r);
        write_files(MockEngine::WireMock, &c, None, &dir).unwrap();
        let text = std::fs::read_to_string(dir.join("mappings/rivet.json")).unwrap();
        assert!(text.contains("\"urlPath\": \"/a\""));
        std::fs::remove_dir_all(&dir).ok();
    }
}
