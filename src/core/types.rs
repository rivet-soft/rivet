//! Shared, UI-free data model. Everything persisted is `Serialize`/`Deserialize`;
//! every module (HTTP, OpenAPI, store, UI) talks in these types.

use std::time::Duration;

use serde::{Deserialize, Serialize};

pub use crate::model::Method;

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// A row of params / headers / form fields / environment variables.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyValue {
    #[serde(default = "yes")]
    pub enabled: bool,
    pub key: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// In multipart form data, read `value` as a local file path.
    #[serde(default)]
    pub file: bool,
}

fn yes() -> bool {
    true
}

impl KeyValue {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            enabled: true,
            key: key.into(),
            value: value.into(),
            description: String::new(),
            file: false,
        }
    }

    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = d.into();
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyKind {
    #[default]
    None,
    Json,
    Text,
    /// `application/x-www-form-urlencoded` from `Body::form`.
    Form,
    /// `multipart/form-data`, with text and file fields in `Body::form`.
    Multipart,
    /// Raw bytes read from `Body::file_path`.
    Binary,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Body {
    pub kind: BodyKind,
    /// Raw text for Json / Text.
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub form: Vec<KeyValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<std::path::PathBuf>,
}

impl Body {
    pub fn json(text: impl Into<String>) -> Self {
        Self {
            kind: BodyKind::Json,
            text: text.into(),
            form: Vec::new(),
            file_path: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Auth {
    #[default]
    None,
    /// Use the security scheme declared by the OpenAPI spec (bearer token from
    /// the active environment's `token` variable).
    Inherit,
    Bearer {
        token: String,
    },
    Basic {
        username: String,
        password: String,
    },
    ApiKey {
        key: String,
        value: String,
        in_query: bool,
    },
}

/// Which OpenAPI operation a request was created from.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpecRef {
    pub method: Method,
    /// Path template as written in the spec, e.g. `/orders/{id}`.
    pub path: String,
}

/// Everything needed to send a request. `{{vars}}` are unresolved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestSpec {
    pub id: String,
    pub name: String,
    pub method: Method,
    /// May contain `{{vars}}` and a query string; `params` are appended.
    pub url: String,
    #[serde(default)]
    pub params: Vec<KeyValue>,
    /// Values of `:name` path segments of the URL (Postman's Path Variables),
    /// in URL order; kept in sync with the URL by `vars::sync_path_vars`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path_vars: Vec<KeyValue>,
    #[serde(default)]
    pub headers: Vec<KeyValue>,
    #[serde(default)]
    pub body: Body,
    #[serde(default)]
    pub auth: Auth,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_ref: Option<SpecRef>,
    /// Owning collection, used for scoped variables and linked specifications.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection_id: Option<String>,
    /// Folder path inside the collection (for example `Users / Admin`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Overrides for the application's redirect defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_redirects: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_redirects: Option<u32>,
    /// Pre-request / post-response JavaScript (Postman-compatible `pm.*`).
    #[serde(default, skip_serializing_if = "Scripts::is_empty")]
    pub scripts: Scripts,
    /// HTTP request or WebSocket connection (URL/headers/auth/params are the handshake).
    #[serde(default, skip_serializing_if = "RequestKind::is_http")]
    pub kind: RequestKind,
    /// WebSocket-only settings: saved message templates.
    #[serde(default, skip_serializing_if = "WsSettings::is_empty")]
    pub ws: WsSettings,
}

/// What a [`RequestSpec`] sends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    #[default]
    Http,
    WebSocket,
}

impl RequestKind {
    pub fn is_http(&self) -> bool {
        *self == RequestKind::Http
    }
}

/// A saved WebSocket message.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WsTemplate {
    pub name: String,
    #[serde(default)]
    pub body: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WsSettings {
    #[serde(default)]
    pub messages: Vec<WsTemplate>,
}

impl WsSettings {
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

impl RequestSpec {
    pub fn new(name: impl Into<String>, method: Method, url: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            method,
            url: url.into(),
            params: Vec::new(),
            path_vars: Vec::new(),
            headers: Vec::new(),
            body: Body::default(),
            auth: Auth::None,
            spec_ref: None,
            collection_id: None,
            folder: None,
            follow_redirects: None,
            max_redirects: None,
            scripts: Scripts::default(),
            kind: RequestKind::Http,
            ws: WsSettings::default(),
        }
    }

    pub fn untitled() -> Self {
        Self::new("Untitled", Method::Get, "")
    }

    /// An empty WebSocket request.
    pub fn untitled_ws() -> Self {
        let mut r = Self::new("Untitled", Method::Get, "");
        r.kind = RequestKind::WebSocket;
        r
    }

    pub fn is_ws(&self) -> bool {
        self.kind == RequestKind::WebSocket
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvColor {
    #[default]
    Green,
    Red,
    Amber,
    Blue,
    Purple,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    pub id: String,
    pub name: String,
    pub color: EnvColor,
    /// Destructive requests (DELETE/PUT/PATCH/POST) ask for confirmation.
    #[serde(default)]
    pub production: bool,
    pub vars: Vec<KeyValue>,
}

impl Environment {
    pub fn new(name: impl Into<String>, color: EnvColor) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            color,
            production: false,
            vars: Vec::new(),
        }
    }

    /// Value of an enabled variable.
    pub fn var(&self, key: &str) -> Option<&str> {
        self.vars
            .iter()
            .find(|v| v.enabled && v.key == key)
            .map(|v| v.value.as_str())
    }

    /// `baseUrl` for display (status bar, env picker).
    pub fn base_url(&self) -> &str {
        self.var("baseUrl").unwrap_or("")
    }
}

/// User-saved requests ("Мои коллекции").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub requests: Vec<RequestSpec>,
    /// Explicit folder paths, including empty folders.
    #[serde(default)]
    pub folders: Vec<String>,
    /// Variables shared by requests in this collection. Environment values override them.
    #[serde(default)]
    pub vars: Vec<KeyValue>,
    /// OpenAPI/Swagger source; requests are refreshed from this file on reload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<std::path::PathBuf>,
    /// Use a cookie jar shared by requests in this collection for this app session.
    #[serde(default = "yes")]
    pub cookies_enabled: bool,
}

impl Collection {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            requests: Vec::new(),
            folders: Vec::new(),
            vars: Vec::new(),
            source: None,
            cookies_enabled: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    /// Unix seconds.
    pub at: u64,
    pub env: String,
    /// The request as it was sent (vars unresolved, so it can be replayed).
    pub request: RequestSpec,
    pub status: Option<u16>,
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub proxy: Option<String>,
    #[serde(default = "yes")]
    pub ssl_verify: bool,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    #[serde(default = "yes")]
    pub follow_redirects: bool,
    #[serde(default = "default_max_redirects")]
    pub max_redirects: u32,
    /// Keep cookies between requests during this app session (within each collection).
    #[serde(default = "yes")]
    pub cookies_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mock_server: Option<String>,
    /// HTTP protocol to use (see [`HttpVersionPref`]).
    #[serde(default)]
    pub http_version: HttpVersionPref,
    /// What AI assistants may do through `rivet mcp` (Settings → MCP).
    #[serde(default)]
    pub mcp: McpSettings,
    /// Check GitHub for a new Rivet release on start and every 6 hours.
    #[serde(default = "yes")]
    pub auto_update: bool,
    /// Release the user chose to skip ("Пропустить эту версию"), e.g. `1.4.0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped_version: Option<String>,
    /// Last language picked in the request "Код" panel (`CodeLang::id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_language: Option<String>,
}

/// Permissions of the MCP server (`rivet mcp`), edited in Settings → MCP and
/// re-read by the server on every tool call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpSettings {
    /// Tool calls are refused when off (`initialize` and `tools/list` still work).
    pub enabled: bool,
    /// Only reading tools; sends still run (with `allow_send`) but don't save
    /// variable changes or history.
    pub read_only: bool,
    /// `send_request` / `run_collection` are allowed.
    pub allow_send: bool,
    /// The `reveal_secrets` tool argument is honoured; otherwise secrets stay masked.
    pub reveal_secrets: bool,
}

impl Default for McpSettings {
    fn default() -> Self {
        Self { enabled: true, read_only: false, allow_send: true, reveal_secrets: false }
    }
}

/// Protocol selection for requests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HttpVersionPref {
    /// ALPN over TLS (h2 preferred, HTTP/1.1 fallback); HTTP/1.1 over plain http.
    #[default]
    Auto,
    /// Always HTTP/1.1.
    Http1,
    /// h2 over TLS (ALPN), h2c with prior knowledge over plain http.
    Http2,
}

fn default_timeout() -> u64 {
    30_000
}

fn default_max_redirects() -> u32 {
    10
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            proxy: None,
            ssl_verify: true,
            timeout_ms: default_timeout(),
            follow_redirects: true,
            max_redirects: default_max_redirects(),
            cookies_enabled: true,
            mock_server: None,
            http_version: HttpVersionPref::Auto,
            mcp: McpSettings::default(),
            auto_update: true,
            skipped_version: None,
            code_language: None,
        }
    }
}

/// Whole persisted project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    /// OpenAPI file the "Из спецификации" tree is built from.
    #[serde(default)]
    pub spec_path: Option<std::path::PathBuf>,
    pub environments: Vec<Environment>,
    #[serde(default)]
    pub active_env: usize,
    #[serde(default)]
    pub collections: Vec<Collection>,
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
    #[serde(default)]
    pub settings: Settings,
    /// Requests open as tabs when the app was closed.
    #[serde(default)]
    pub open_tabs: Vec<RequestSpec>,
    #[serde(default)]
    pub active_tab: Option<usize>,
    /// Width of the collections/history sidebar, as the user dragged it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_width: Option<u32>,
}

/// Request after `{{var}}` substitution, params and auth applied: ready to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedRequest {
    pub method: Method,
    /// Absolute URL including the query string.
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    /// Variables referenced but not defined in the environment.
    pub missing_vars: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Timings {
    /// Sum of the earlier redirect hops' totals (see `ResponseData::hops`).
    /// The other phases describe the final hop; `total` covers the chain.
    pub redirect: Duration,
    pub dns: Duration,
    pub connect: Duration,
    pub tls: Duration,
    /// Time to first byte after the request was written.
    pub ttfb: Duration,
    pub download: Duration,
    pub total: Duration,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResponseData {
    pub status: u16,
    pub status_text: String,
    /// "HTTP/1.1", "HTTP/2".
    pub http_version: String,
    pub remote_addr: Option<String>,
    pub headers: Vec<(String, String)>,
    /// URLs visited from the original request through the final response.
    pub redirect_history: Vec<String>,
    /// Redirect responses received before the final one, in order.
    pub hops: Vec<RedirectHop>,
    pub body: Vec<u8>,
    pub timings: Timings,
}

/// One redirect response of a followed chain.
#[derive(Clone, Debug, PartialEq)]
pub struct RedirectHop {
    pub method: Method,
    pub url: String,
    pub status: u16,
    pub status_text: String,
    /// Absolute target URL (Location resolved against `url`).
    pub location: String,
    pub headers: Vec<(String, String)>,
    pub remote_addr: Option<String>,
    pub timings: Timings,
}

impl ResponseData {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn content_type(&self) -> Option<&str> {
        self.header("content-type")
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn is_json(&self) -> bool {
        self.content_type().is_some_and(|c| c.contains("json"))
            || serde_json::from_slice::<serde_json::Value>(&self.body).is_ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendError {
    InvalidUrl(String),
    File {
        path: String,
        message: String,
    },
    /// DNS failure, refused connection, TLS error… `code` like "ECONNREFUSED".
    Network {
        code: String,
        message: String,
        host: String,
    },
    Timeout {
        after_ms: u64,
    },
    Cancelled,
    /// The pre-request script failed; the request was not sent.
    Script {
        message: String,
        line: Option<u32>,
    },
    /// A header name/value HTTP forbids (line break, control char, space in
    /// the name…); the request was not sent.
    InvalidHeader {
        name: String,
        message: String,
    },
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::InvalidUrl(u) => write!(f, "Invalid URL: {u}"),
            SendError::File { path, message } => write!(f, "File {path}: {message}"),
            SendError::Network { code, message, .. } => write!(f, "{code}: {message}"),
            SendError::Timeout { after_ms } => write!(f, "Timed out after {after_ms} ms"),
            SendError::Cancelled => write!(f, "Cancelled"),
            SendError::Script { message, line: Some(l) } => write!(f, "Pre-request script, line {l}: {message}"),
            SendError::Script { message, line: None } => write!(f, "Pre-request script: {message}"),
            SendError::InvalidHeader { name, message } => write!(f, "Header «{name}»: {message}"),
        }
    }
}

/// One difference between a response body and its OpenAPI schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mismatch {
    /// JSON path, e.g. `$.status` or `$.items[0].quantity`.
    pub path: String,
    pub title: String,
    pub expected: String,
    pub actual: String,
    /// 0-based line in the pretty-printed body, when known.
    pub line: Option<usize>,
}

/// One operation from the OpenAPI spec (sidebar "Из спецификации").
#[derive(Clone, Debug, PartialEq)]
pub struct SpecEndpoint {
    pub method: Method,
    pub path: String,
    pub name: String,
    pub description: String,
    pub params: Vec<KeyValue>,
    /// Example JSON body generated from the request schema, if any.
    pub example_body: Option<String>,
    /// Bearer/basic/apiKey from `security`, as declared.
    pub requires_auth: bool,
}

impl SpecEndpoint {
    /// A fresh request for this operation, URL based on `{{baseUrl}}`.
    pub fn to_request(&self) -> RequestSpec {
        let mut r = RequestSpec::new(
            self.name.clone(),
            self.method,
            format!("{{{{baseUrl}}}}{}", self.path),
        );
        r.params = self.params.clone();
        if let Some(body) = &self.example_body {
            r.body = Body::json(body.clone());
        }
        if self.requires_auth {
            r.auth = Auth::Inherit;
        }
        r.spec_ref = Some(SpecRef {
            method: self.method,
            path: self.path.clone(),
        });
        r
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpecGroup {
    pub name: String,
    pub endpoints: Vec<SpecEndpoint>,
}


// ---- scripts -----------------------------------------------------------------

/// Request scripts. Empty strings mean "no script".
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scripts {
    #[serde(default)]
    pub pre_request: String,
    #[serde(default)]
    pub post_response: String,
}

impl Scripts {
    pub fn is_empty(&self) -> bool {
        self.pre_request.trim().is_empty() && self.post_response.trim().is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptStage {
    PreRequest,
    PostResponse,
}

/// Everything a script can see. Variables are the effective values for the
/// request (environment merged with collection variables, like `vars::resolve`).
#[derive(Clone, Debug)]
pub struct ScriptInput {
    pub stage: ScriptStage,
    pub request: RequestSpec,
    pub env: Vec<KeyValue>,
    pub collection_vars: Vec<KeyValue>,
    /// Only for `PostResponse`.
    pub response: Option<ResponseData>,
}

/// `set` (value Some) or `unset` (value None) of one variable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VarChange {
    pub key: String,
    pub value: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    /// Assertion message for failures.
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Log,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    pub level: LogLevel,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptError {
    pub message: String,
    /// 1-based line in the user's script, when known.
    pub line: Option<u32>,
}

/// Result of running one script (`core::script::run`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptOutcome {
    /// The request after a pre-request script (unchanged for post-response).
    pub request: Option<RequestSpec>,
    pub env_changes: Vec<VarChange>,
    pub collection_changes: Vec<VarChange>,
    pub tests: Vec<TestResult>,
    pub logs: Vec<LogLine>,
    pub error: Option<ScriptError>,
    pub duration: std::time::Duration,
}

/// Both stages of one send, as shown in the response panel (Tests / Console).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptReport {
    pub pre: Option<ScriptOutcome>,
    pub post: Option<ScriptOutcome>,
}

impl ScriptReport {
    pub fn is_empty(&self) -> bool {
        self.pre.is_none() && self.post.is_none()
    }

    pub fn tests(&self) -> impl Iterator<Item = &TestResult> {
        self.pre.iter().chain(self.post.iter()).flat_map(|o| o.tests.iter())
    }

    pub fn logs(&self) -> impl Iterator<Item = (ScriptStage, &LogLine)> {
        let pre = self.pre.iter().flat_map(|o| o.logs.iter().map(|l| (ScriptStage::PreRequest, l)));
        let post = self.post.iter().flat_map(|o| o.logs.iter().map(|l| (ScriptStage::PostResponse, l)));
        pre.chain(post)
    }

    pub fn errors(&self) -> impl Iterator<Item = (ScriptStage, &ScriptError)> {
        let pre = self.pre.iter().filter_map(|o| o.error.as_ref().map(|e| (ScriptStage::PreRequest, e)));
        let post = self.post.iter().filter_map(|o| o.error.as_ref().map(|e| (ScriptStage::PostResponse, e)));
        pre.chain(post)
    }
}
