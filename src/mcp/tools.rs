//! MCP tools: definitions (JSON Schemas) and handlers. Every call reloads
//! `project.json`; mutating tools save it atomically before returning.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::args::{self, Args};
use super::output;
use crate::core::collection_run::{self, CollectionRunOptions};
use crate::core::export::{self, ExportFormat};
use crate::core::http::{CancelToken, HttpSessions};
use crate::core::project_ops::{self, find_collection, find_env, find_request, is_destructive};
use crate::core::mcp_activity::{self, ActivityEntry};
use crate::core::secrets;
use crate::core::{
    Collection, EnvColor, Environment, KeyValue, McpSettings, Project, RequestSpec, import, runner, store,
};

/// Default cap for response bodies returned to the model.
const DEFAULT_MAX_BODY_CHARS: usize = 20_000;

/// A tool's successful (or soft-failed) result.
pub struct Reply {
    pub value: Value,
    pub is_error: bool,
}

impl From<Value> for Reply {
    fn from(value: Value) -> Self {
        Reply { value, is_error: false }
    }
}

type ToolResult = Result<Reply, String>;

/// Server state kept between calls: the project path, the cookie jars of this
/// session, the client's name and the MCP permissions read for the current
/// call — never a copy of the project.
pub struct Server {
    path: PathBuf,
    sessions: HttpSessions,
    client: Option<String>,
    policy: McpSettings,
}

/// Tool error while MCP is switched off in the app.
pub const DISABLED_MESSAGE: &str = "MCP отключён в настройках Rivet (Настройки → MCP)";

/// Tools that change the project.
const MUTATING_TOOLS: [&str; 6] =
    ["save_request", "set_variable", "unset_variable", "set_active_environment", "create_environment", "import_spec"];
/// Tools that send HTTP requests.
const SENDING_TOOLS: [&str; 2] = ["send_request", "run_collection"];

const READ_ONLY_NOTE: &str = "Rivet MCP is in read-only mode (Settings → MCP): variable changes and history were not saved.";

const REVEAL: &str = "Return secret-looking values (tokens, passwords, API keys, Authorization) in clear text. Default false: they are masked as ••••••••.";

fn reveal_prop() -> Value {
    json!({"type": "boolean", "description": REVEAL})
}

fn request_props() -> serde_json::Map<String, Value> {
    let kv = |what: &str| {
        json!({
            "description": format!("{what}: array of {{key, value, enabled?, description?}} or an object {{\"name\": \"value\"}}. Replaces the whole list."),
            "anyOf": [
                {"type": "array", "items": {"type": "object", "properties": {
                    "key": {"type": "string"}, "value": {"type": "string"},
                    "enabled": {"type": "boolean"}, "description": {"type": "string"},
                    "file": {"type": "boolean", "description": "Multipart only: value is a local file path"}
                }, "required": ["key"]}},
                {"type": "object", "additionalProperties": {"type": "string"}}
            ]
        })
    };
    let props = json!({
        "name": {"type": "string", "description": "Request name shown in the sidebar"},
        "method": {"type": "string", "enum": ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"]},
        "url": {"type": "string", "description": "URL; may contain {{variables}} (e.g. {{baseUrl}}/orders) and a query string"},
        "params": kv("Query parameters appended to the URL"),
        "headers": kv("Request headers"),
        "body": {
            "description": "Body. A JSON object/array is sent as JSON; a string is sent as JSON if it parses, else as text; or {kind: none|json|text|form|multipart|binary, text?, form?, file_path?}.",
            "anyOf": [{"type": "object"}, {"type": "array"}, {"type": "string"}, {"type": "null"}]
        },
        "auth": {
            "type": "object",
            "description": "{type: none} | {type: inherit} (bearer from the environment's `token`) | {type: bearer, token} | {type: basic, username, password} | {type: api_key, key, value, in_query?}",
            "properties": {
                "type": {"type": "string", "enum": ["none", "inherit", "bearer", "basic", "api_key"]},
                "token": {"type": "string"}, "username": {"type": "string"}, "password": {"type": "string"},
                "key": {"type": "string"}, "value": {"type": "string"}, "in_query": {"type": "boolean"}
            },
            "required": ["type"]
        },
        "scripts": {
            "type": "object",
            "description": "Postman-compatible JavaScript (pm.*): pre_request runs before sending, post_response after (pm.test, pm.expect, pm.environment.set, pm.collectionVariables.set…). Omitted keys keep their value.",
            "properties": {"pre_request": {"type": "string"}, "post_response": {"type": "string"}}
        },
        "follow_redirects": {"type": ["boolean", "null"], "description": "Override the app's redirect setting for this request (null = use the app default)"},
        "max_redirects": {"type": ["integer", "null"], "minimum": 0}
    });
    props.as_object().cloned().unwrap_or_default()
}

fn tool(name: &str, title: &str, description: &str, properties: Value, required: &[&str], read_only: bool) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": {"type": "object", "properties": properties, "required": required},
        "annotations": {"title": title, "readOnlyHint": read_only, "openWorldHint": false},
    })
}

/// All tool definitions for `tools/list`.
pub fn definitions() -> Vec<Value> {
    let scope = json!({"type": "string", "enum": ["environment", "collection"], "description": "Variable scope"});
    let scope_name = json!({"type": "string", "description": "Environment or collection id/name. Optional for environments (defaults to the active one), required for collections."});

    let mut save = request_props();
    save.insert("collection".into(), json!({"type": "string", "description": "Collection id or name. Required when creating; when updating, moves the request there. A name that matches no collection creates a new collection."}));
    save.insert("id".into(), json!({"type": "string", "description": "Id of an existing request to update (only the given fields change). Omit to create a new request."}));
    save.insert("folder".into(), json!({"type": "string", "description": "Folder path inside the collection, e.g. \"Users\" or \"Users / Admin\"; empty string = collection root."}));

    let mut send = request_props();
    send.insert("request_id".into(), json!({"type": "string", "description": "Saved request to send (id, unique name or \"Collection / Folder / Name\"). Request fields given alongside override the saved ones for this send only."}));
    send.insert("collection".into(), json!({"type": "string", "description": "For ad-hoc requests: run in this collection's scope (its variables and cookies)."}));
    send.insert("environment".into(), json!({"type": "string", "description": "Environment id/name to use instead of the active one"}));
    send.insert("timeout_ms".into(), json!({"type": "integer", "minimum": 1, "description": "Request timeout (default: app setting, 30000)"}));
    send.insert("max_body_chars".into(), json!({"type": "integer", "minimum": 0, "description": "Truncate the response body to this many characters (default 20000)"}));
    send.insert("confirm_production".into(), json!({"type": "boolean", "description": "Required for POST/PUT/PATCH/DELETE when the environment is marked production. Ask the user before setting it."}));
    send.insert("save_history".into(), json!({"type": "boolean", "description": "Record the send in Rivet's history (default true)"}));
    send.insert("reveal_secrets".into(), reveal_prop());

    vec![
        tool(
            "list_collections",
            "List collections",
            "List Rivet collections with ids, linked spec file, folders and request counts.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "list_requests",
            "List requests",
            "List saved requests (id, name, method, URL template, collection, folder). Optionally filter by collection, folder or a search query, and include the operations of the project's OpenAPI spec.",
            json!({
                "collection": {"type": "string", "description": "Collection id or name"},
                "folder": {"type": "string", "description": "Folder path (includes subfolders)"},
                "query": {"type": "string", "description": "Case-insensitive match on name, URL or method"},
                "include_spec_endpoints": {"type": "boolean", "description": "Also list operations of the project's OpenAPI spec (\"Из спецификации\")"}
            }),
            &[],
            true,
        ),
        tool(
            "get_request",
            "Get request",
            "Full definition of a saved request (method, URL, params, headers, body, auth, scripts…). Secrets are masked unless reveal_secrets is true; masked values sent back to save_request keep the stored secret.",
            json!({
                "id": {"type": "string", "description": "Request id, unique name or \"Collection / Folder / Name\""},
                "collection": {"type": "string", "description": "Narrow a name lookup to this collection"},
                "reveal_secrets": reveal_prop()
            }),
            &["id"],
            true,
        ),
        tool(
            "save_request",
            "Save request",
            "Create a request in a collection or update an existing one (pass id). Returns the request id.",
            Value::Object(save),
            &[],
            false,
        ),
        tool(
            "send_request",
            "Send request",
            "Send a saved request (request_id) or an ad-hoc one (method + url…). Runs the pre-request script, resolves {{variables}}, sends over HTTP/1.1 or HTTP/2, runs the post-response script. Returns status, headers, body (truncated), timings, redirects, test results, console logs and schema mismatches; variable changes made by scripts and a history entry are saved like in the app.",
            Value::Object(send),
            &[],
            false,
        ),
        tool(
            "get_history",
            "Get history",
            "Recent sends from Rivet's history, newest first.",
            json!({
                "limit": {"type": "integer", "minimum": 1, "maximum": 500, "description": "Entries to return (default 20)"},
                "query": {"type": "string", "description": "Case-insensitive match on URL, name or method"},
                "include_request": {"type": "boolean", "description": "Include the full sent request of each entry"},
                "reveal_secrets": reveal_prop()
            }),
            &[],
            true,
        ),
        tool(
            "list_environments",
            "List environments",
            "Environments with id, name, production flag, baseUrl and which one is active.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "set_active_environment",
            "Set active environment",
            "Make an environment active (used by requests and the app).",
            json!({"name": {"type": "string", "description": "Environment id or name"}}),
            &["name"],
            false,
        ),
        tool(
            "create_environment",
            "Create environment",
            "Create an environment (e.g. Dev, Stage, Prod) with optional variables such as baseUrl and token. Mark it production to require confirm_production for destructive requests.",
            json!({
                "name": {"type": "string", "description": "Environment name; must not match an existing one"},
                "color": {"type": "string", "enum": ["green", "red", "amber", "blue", "purple"], "description": "Dot color in the app (default: red for production, green otherwise)"},
                "production": {"type": "boolean", "description": "Destructive requests (POST/PUT/PATCH/DELETE) then need confirm_production: true. Default false"},
                "variables": {
                    "description": "Variables: array of {key, value, enabled?, description?} or an object {\"name\": \"value\"}",
                    "anyOf": [
                        {"type": "array", "items": {"type": "object", "properties": {
                            "key": {"type": "string"}, "value": {"type": "string"},
                            "enabled": {"type": "boolean"}, "description": {"type": "string"}
                        }, "required": ["key"]}},
                        {"type": "object", "additionalProperties": {"type": "string"}}
                    ]
                },
                "activate": {"type": "boolean", "description": "Make it the active environment (default false; the first environment is always active)"}
            }),
            &["name"],
            false,
        ),
        tool(
            "get_variables",
            "Get variables",
            "Variables of an environment or a collection. Collection variables override environment ones when requests resolve {{vars}}.",
            json!({"scope": scope, "name": scope_name, "reveal_secrets": reveal_prop()}),
            &["scope"],
            true,
        ),
        tool(
            "set_variable",
            "Set variable",
            "Create or update a variable in an environment or a collection.",
            json!({
                "scope": scope, "name": scope_name,
                "key": {"type": "string"}, "value": {"type": "string"},
                "enabled": {"type": "boolean", "description": "Default true"}
            }),
            &["scope", "key", "value"],
            false,
        ),
        tool(
            "unset_variable",
            "Unset variable",
            "Remove a variable from an environment or a collection.",
            json!({"scope": scope, "name": scope_name, "key": {"type": "string"}}),
            &["scope", "key"],
            false,
        ),
        tool(
            "import_spec",
            "Import spec",
            "Import an OpenAPI 3 / Swagger 2 file (YAML or JSON), a Postman v2.x collection, a HAR capture (requests with recorded responses as examples) or a Rivet export into a new collection. OpenAPI collections stay linked to the file.",
            json!({"path": {"type": "string", "description": "Absolute path of the file to import"}}),
            &["path"],
            false,
        ),
        tool(
            "export_collection",
            "Export collection",
            "Export a collection as Rivet (lossless, re-importable), Postman Collection v2.1 or OpenAPI 3.0 YAML. With `path` the file is written there, otherwise the content is returned. Secrets are masked unless reveal_secrets (and Settings → MCP allows it). Warnings list what the format can't carry.",
            json!({
                "collection": {"type": "string", "description": "Collection id or name"},
                "format": {"type": "string", "enum": ["rivet", "postman", "openapi"], "description": "Default rivet"},
                "path": {"type": "string", "description": "Absolute file path to write; omit to get the content back"},
                "reveal_secrets": reveal_prop()
            }),
            &["collection"],
            false,
        ),
        tool(
            "run_collection",
            "Run collection",
            "Run a collection's requests (or one folder) in order, carrying variables set by scripts to the next request. Returns per-request status, tests and errors plus totals; variable changes and history are saved.",
            json!({
                "collection": {"type": "string", "description": "Collection id or name"},
                "folder": {"type": "string", "description": "Only this folder (and subfolders)"},
                "environment": {"type": "string", "description": "Environment id/name instead of the active one"},
                "stop_on_failure": {"type": "boolean", "description": "Stop after the first error or failed test"},
                "delay_ms": {"type": "integer", "minimum": 0, "description": "Pause between requests"},
                "confirm_production": {"type": "boolean", "description": "Required when the environment is production and the run contains POST/PUT/PATCH/DELETE. Ask the user first."},
                "reveal_secrets": reveal_prop()
            }),
            &["collection"],
            false,
        ),
    ]
}

/// Names of all tools, in `tools/list` order.
pub fn tool_names() -> &'static [&'static str] {
    &[
        "list_collections",
        "list_requests",
        "get_request",
        "save_request",
        "send_request",
        "get_history",
        "list_environments",
        "set_active_environment",
        "create_environment",
        "get_variables",
        "set_variable",
        "unset_variable",
        "import_spec",
        "export_collection",
        "run_collection",
    ]
}

fn production_refusal(env: &Environment, what: &str) -> String {
    format!(
        "Environment '{}' is marked as production. {what} can change real data, so it needs explicit confirmation: ask the user, then call again with confirm_production: true.",
        env.name
    )
}

impl Server {
    pub fn new(path: PathBuf) -> Self {
        Self { path, sessions: HttpSessions::default(), client: None, policy: McpSettings::default() }
    }

    /// Remembers `clientInfo.name` from `initialize` for the activity log.
    pub fn set_client(&mut self, name: Option<String>) {
        self.client = name.filter(|n| !n.trim().is_empty());
    }

    /// `reveal_secrets` is honoured only when Settings → MCP allows it.
    fn reveal(&self, args: &Args) -> Result<bool, String> {
        Ok(self.policy.reveal_secrets && args.flag("reveal_secrets")?)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Fresh copy of the project file (an empty project when it doesn't exist).
    /// A corrupt file is an error: the server never overwrites it.
    fn load(&self) -> Result<Project, String> {
        if !self.path.exists() {
            return Ok(store::seed_project());
        }
        store::load(&self.path).map_err(|e| format!("Cannot read the Rivet project: {e}"))
    }

    /// Writes `local` (the result of editing `base`, a fresh load) atomically.
    /// When the file changed meanwhile (the app saved), the three versions are
    /// merged with the file winning conflicts.
    fn save(&self, base: &Project, local: &Project) -> Result<(), String> {
        let disk = if self.path.exists() { Some(self.load()?) } else { None };
        let project = match disk {
            Some(disk) if disk != *base => store::merge_projects(base, local, &disk),
            _ => local.clone(),
        };
        store::save(&self.path, &project).map_err(|e| format!("Cannot save the Rivet project: {e}"))
    }

    /// Runs one tool under the permissions in Settings → MCP and records it in
    /// the activity log. `Err` is a tool error with a readable message.
    pub fn call(&mut self, name: &str, arguments: &Value) -> ToolResult {
        let result = self.call_checked(name, arguments);
        let (ok, summary) = match &result {
            Ok(reply) => (!reply.is_error, summarize(name, &reply.value)),
            Err(message) => (false, message.clone()),
        };
        let entry = ActivityEntry::new(name, ok, summary, self.client.clone());
        if let Err(e) = mcp_activity::append(&mcp_activity::activity_path(&self.path), &entry) {
            eprintln!("rivet mcp: activity log: {e}");
        }
        result
    }

    fn call_checked(&mut self, name: &str, arguments: &Value) -> ToolResult {
        self.policy = self.load()?.settings.mcp;
        if !self.policy.enabled {
            return Err(DISABLED_MESSAGE.into());
        }
        if self.policy.read_only && MUTATING_TOOLS.contains(&name) {
            return Err(format!(
                "Rivet MCP работает в режиме «Только чтение» (Настройки → MCP): {name} недоступен. Read-only mode: this tool changes the project and is disabled."
            ));
        }
        if !self.policy.allow_send && SENDING_TOOLS.contains(&name) {
            return Err(format!(
                "Отправка запросов через MCP запрещена в настройках Rivet (Настройки → MCP → «Разрешить отправку запросов»): {name} недоступен."
            ));
        }
        let args = Args::new(arguments);
        match name {
            "list_collections" => self.list_collections(),
            "list_requests" => self.list_requests(&args),
            "get_request" => self.get_request(&args),
            "save_request" => self.save_request(&args),
            "send_request" => self.send_request(&args),
            "get_history" => self.get_history(&args),
            "list_environments" => self.list_environments(),
            "set_active_environment" => self.set_active_environment(&args),
            "create_environment" => self.create_environment(&args),
            "get_variables" => self.get_variables(&args),
            "set_variable" => self.set_variable(&args, true),
            "unset_variable" => self.set_variable(&args, false),
            "import_spec" => self.import_spec(&args),
            "export_collection" => self.export_collection(&args),
            "run_collection" => self.run_collection(&args),
            other => Err(format!("Unknown tool: {other}")),
        }
    }

    fn list_collections(&self) -> ToolResult {
        let project = self.load()?;
        let collections: Vec<Value> = project
            .collections
            .iter()
            .map(|c| {
                let mut folders: Vec<String> = c.folders.clone();
                for r in &c.requests {
                    if let Some(f) = &r.folder {
                        if !folders.contains(f) {
                            folders.push(f.clone());
                        }
                    }
                }
                let folders: Vec<Value> = folders
                    .iter()
                    .map(|f| json!({"path": f, "requests": c.requests.iter().filter(|r| r.folder.as_deref() == Some(f)).count()}))
                    .collect();
                json!({
                    "id": c.id,
                    "name": c.name,
                    "source_file": c.source.as_ref().map(|p| p.display().to_string()),
                    "request_count": c.requests.len(),
                    "root_requests": c.requests.iter().filter(|r| r.folder.is_none()).count(),
                    "folders": folders,
                    "variable_count": c.vars.len(),
                })
            })
            .collect();
        Ok(json!({
            "project_file": self.path.display().to_string(),
            "spec_file": project.spec_path.as_ref().map(|p| p.display().to_string()),
            "collections": collections,
        })
        .into())
    }

    fn list_requests(&self, args: &Args) -> ToolResult {
        let project = self.load()?;
        let collections: Vec<&Collection> = match args.str("collection")? {
            Some(c) => vec![&project.collections[find_collection(&project, c)?]],
            None => project.collections.iter().collect(),
        };
        let folder = args.str("folder")?;
        let query = args.str("query")?.map(str::to_lowercase);
        let matches = |name: &str, url: &str, method: &str| {
            query.as_ref().is_none_or(|q| {
                name.to_lowercase().contains(q) || url.to_lowercase().contains(q) || method.to_lowercase() == *q
            })
        };
        let mut requests = Vec::new();
        for c in collections {
            for r in &c.requests {
                if folder.is_some_and(|f| !project_ops::in_folder(r, f)) || !matches(&r.name, &r.url, r.method.full_label()) {
                    continue;
                }
                let mut item = json!({
                    "id": r.id,
                    "name": r.name,
                    "method": r.method.full_label(),
                    "url": output::mask_url(&r.url, false),
                    "collection": c.name,
                    "collection_id": c.id,
                    "folder": r.folder,
                });
                if r.is_ws() {
                    item["kind"] = json!("websocket");
                }
                if let Some(s) = &r.spec_ref {
                    item["spec_operation"] = json!(format!("{} {}", s.method.full_label(), s.path));
                }
                requests.push(item);
            }
        }
        let mut out = json!({"count": requests.len(), "requests": requests});
        if args.flag("include_spec_endpoints")? {
            out["spec_endpoints"] = match &project.spec_path {
                None => json!([]),
                Some(path) => {
                    let spec = crate::core::openapi::Spec::load(path)?;
                    Value::Array(
                        spec.groups
                            .iter()
                            .flat_map(|g| g.endpoints.iter().map(move |e| (g, e)))
                            .filter(|(_, e)| matches(&e.name, &e.path, e.method.full_label()))
                            .map(|(g, e)| {
                                json!({"group": g.name, "method": e.method.full_label(), "path": e.path, "name": e.name, "requires_auth": e.requires_auth})
                            })
                            .collect(),
                    )
                }
            };
        }
        Ok(out.into())
    }

    fn get_request(&self, args: &Args) -> ToolResult {
        let project = self.load()?;
        let (ci, ri) = find_request(&project, args.req_str("id")?, args.str("collection")?)?;
        let c = &project.collections[ci];
        Ok(json!({
            "collection": c.name,
            "collection_id": c.id,
            "request": output::request(&c.requests[ri], self.reveal(args)?),
        })
        .into())
    }

    fn save_request(&self, args: &Args) -> ToolResult {
        let base = self.load()?;
        let mut project = base.clone();
        let target = match args.str("collection")? {
            Some(c) => Some(c),
            None if args.has("id") => None,
            None => {
                let names: Vec<&str> = project.collections.iter().map(|c| c.name.as_str()).collect();
                return Err(format!(
                    "'collection' is required to create a request. Existing collections: {}",
                    if names.is_empty() { "(none — pass a new name to create one)".into() } else { names.join(", ") }
                ));
            }
        };
        let mut collection_created = false;
        let mut target_ix = match target {
            Some(t) => match find_collection(&project, t) {
                Ok(ix) => Some(ix),
                Err(e) if e.contains("not found") => {
                    project.collections.push(Collection::new(t.trim()));
                    collection_created = true;
                    Some(project.collections.len() - 1)
                }
                Err(e) => return Err(e),
            },
            None => None,
        };

        let (mut request, previous, created) = match args.str("id")? {
            Some(id) => {
                let (ci, ri) = project
                    .collections
                    .iter()
                    .enumerate()
                    .find_map(|(ci, c)| c.requests.iter().position(|r| r.id == id).map(|ri| (ci, ri)))
                    .ok_or_else(|| format!("No saved request with id '{id}' (omit id to create a new one)"))?;
                let previous = project.collections[ci].requests.remove(ri);
                target_ix.get_or_insert(ci);
                (previous.clone(), Some((previous, ci, ri)), false)
            }
            None => (RequestSpec::untitled(), None, true),
        };
        args::apply_request_fields(&mut request, args)?;
        if let Some((old, _, _)) = &previous {
            secrets::unmask_request(&mut request, old);
        }
        if created && !args.has("name") {
            request.name = args::name_from_url(&request.url);
        }
        let ci = target_ix.unwrap_or(0);
        let collection = &mut project.collections[ci];
        request.collection_id = Some(collection.id.clone());
        if let Some(folder) = args.get("folder") {
            request.folder = args::normalize_folder(folder.as_str().ok_or("'folder' must be a string")?);
        }
        if let Some(folder) = &request.folder {
            // Register the folder and its ancestors so the sidebar shows them.
            let parts: Vec<&str> = folder.split(project_ops::FOLDER_SEP).collect();
            for depth in 1..=parts.len() {
                let path = parts[..depth].join(project_ops::FOLDER_SEP);
                if !collection.folders.contains(&path) {
                    collection.folders.push(path);
                }
            }
        }
        let id = request.id.clone();
        // Keep the position when updating in place; append otherwise.
        match previous {
            Some((_, old_ci, ri)) if old_ci == ci => collection.requests.insert(ri, request),
            _ => collection.requests.push(request),
        }
        let (collection_id, collection_name) = (collection.id.clone(), collection.name.clone());
        self.save(&base, &project)?;
        Ok(json!({
            "id": id,
            "created": created,
            "collection_id": collection_id,
            "collection": collection_name,
            "collection_created": collection_created,
        })
        .into())
    }

    fn send_request(&self, args: &Args) -> ToolResult {
        let project = self.load()?;
        let reveal = self.reveal(args)?;
        let mut request = match args.str("request_id")? {
            Some(id) => {
                let (ci, ri) = find_request(&project, id, args.str("collection")?)?;
                let mut r = project.collections[ci].requests[ri].clone();
                r.collection_id = Some(project.collections[ci].id.clone());
                r
            }
            None => {
                if !args.has("url") {
                    return Err("Pass either request_id (a saved request) or at least url (an ad-hoc request)".into());
                }
                let mut r = RequestSpec::untitled();
                if let Some(c) = args.str("collection")? {
                    r.collection_id = Some(project.collections[find_collection(&project, c)?].id.clone());
                }
                r
            }
        };
        let saved = request.clone();
        args::apply_request_fields(&mut request, args)?;
        secrets::unmask_request(&mut request, &saved);
        if request.is_ws() {
            return Err("WebSocket requests can't be sent through MCP; only HTTP requests are supported".into());
        }
        if request.url.trim().is_empty() {
            return Err("The request has no URL".into());
        }
        if args.str("name")?.is_none() && args.str("request_id")?.is_none() {
            request.name = args::name_from_url(&request.url);
        }

        let env = match args.str("environment")? {
            Some(name) => Some(project.environments[find_env(&project, name)?].clone()),
            None => project_ops::active_env(&project).cloned(),
        };
        if let Some(env) = env.as_ref().filter(|e| e.production) {
            if is_destructive(request.method) && !args.flag("confirm_production")? {
                return Err(production_refusal(env, &format!("{} {}", request.method.full_label(), request.url)));
            }
        }

        let cancel = CancelToken::new();
        let mut input = project_ops::run_input(&project, &request, env.clone(), &cancel, Some(&self.sessions));
        if let Some(follow) = args.bool("follow_redirects")? {
            input.settings.follow_redirects = follow;
        }
        if let Some(timeout) = args.u64("timeout_ms")? {
            input.settings.timeout_ms = timeout.max(1);
        }
        let out = runner::run(input);

        // Persist on a fresh copy: the send may have taken a while. Nothing is
        // saved in read-only mode.
        let persist = !self.policy.read_only;
        if persist {
            let base = self.load()?;
            let mut fresh = base.clone();
            let env_id = env.as_ref().map(|e| e.id.as_str());
            let collection_id = out.sent.collection_id.as_deref();
            project_ops::apply_var_changes(&mut fresh, collection_id, env_id, &out.env_changes, &out.collection_changes);
            let (status, duration_ms) = match &out.result {
                Ok(r) => (Some(r.status), Some(r.timings.total.as_millis() as u64)),
                Err(_) => (None, None),
            };
            if args.bool("save_history")?.unwrap_or(true) {
                let env_name = env.as_ref().map(|e| e.name.as_str()).unwrap_or("");
                store::push_history(&mut fresh, project_ops::history_entry(env_name, out.sent.clone(), status, duration_ms));
            }
            if fresh != base || !self.path.exists() {
                self.save(&base, &fresh)?;
            }
        }

        let max_chars = args.u64("max_body_chars")?.map_or(DEFAULT_MAX_BODY_CHARS, |n| n as usize);
        let mut value = json!({
            "request": {
                "id": (!out.sent.id.is_empty() && args.has("request_id")).then(|| out.sent.id.clone()),
                "name": out.sent.name,
                "method": out.sent.method.full_label(),
                "url": output::mask_url(out.resolved_url.as_deref().unwrap_or(&out.sent.url), reveal),
            },
            "environment": env.as_ref().map(|e| e.name.clone()),
        });
        let is_error = match &out.result {
            Ok(response) => {
                value["response"] = output::response(response, max_chars, reveal);
                if out.sent.spec_ref.is_some() {
                    if let Some(spec) = project_ops::spec_for_request(&project, &out.sent) {
                        let (label, mismatches) = project_ops::check_schema(&spec, &out.sent, response);
                        value["schema"] = json!({"label": label, "mismatches": output::mismatches(&mismatches)});
                    }
                }
                false
            }
            Err(e) => {
                value["error"] = json!(e.to_string());
                true
            }
        };
        if !out.missing_vars.is_empty() {
            value["undefined_variables"] = json!(out.missing_vars);
            value["warning"] = json!(format!(
                "These {{{{variables}}}} are defined in neither the environment nor the collection and were sent as empty strings: {}",
                out.missing_vars.join(", ")
            ));
        }
        value["scripts"] = output::script_report(&out.report);
        value["variable_changes"] = json!({
            "environment": output::var_changes(&out.env_changes, reveal),
            "collection": output::var_changes(&out.collection_changes, reveal),
        });
        if !persist {
            value["persisted"] = json!(false);
            value["note"] = json!(READ_ONLY_NOTE);
        }
        Ok(Reply { value, is_error })
    }

    fn get_history(&self, args: &Args) -> ToolResult {
        let project = self.load()?;
        let limit = args.u64("limit")?.unwrap_or(20).clamp(1, 500) as usize;
        let query = args.str("query")?.map(str::to_lowercase);
        let reveal = self.reveal(args)?;
        let include_request = args.flag("include_request")?;
        let entries: Vec<Value> = project
            .history
            .iter()
            .filter(|h| {
                query.as_ref().is_none_or(|q| {
                    let r = &h.request;
                    r.url.to_lowercase().contains(q)
                        || r.name.to_lowercase().contains(q)
                        || r.method.full_label().eq_ignore_ascii_case(q)
                })
            })
            .take(limit)
            .map(|h| {
                let mut e = json!({
                    "id": h.id,
                    "at": h.at,
                    "environment": h.env,
                    "request_id": h.request.id,
                    "name": h.request.name,
                    "method": h.request.method.full_label(),
                    "url": output::mask_url(&h.request.url, reveal),
                    "status": h.status,
                    "duration_ms": h.duration_ms,
                });
                if include_request {
                    e["request"] = output::request(&h.request, reveal);
                }
                e
            })
            .collect();
        Ok(json!({"total": project.history.len(), "entries": entries}).into())
    }

    fn list_environments(&self) -> ToolResult {
        let project = self.load()?;
        let envs: Vec<Value> = project
            .environments
            .iter()
            .enumerate()
            .map(|(i, e)| {
                json!({
                    "id": e.id,
                    "name": e.name,
                    "active": i == project.active_env,
                    "production": e.production,
                    "base_url": e.base_url(),
                    "variable_count": e.vars.len(),
                })
            })
            .collect();
        Ok(json!({
            "active": project_ops::active_env(&project).map(|e| e.name.clone()),
            "environments": envs,
        })
        .into())
    }

    fn set_active_environment(&self, args: &Args) -> ToolResult {
        let base = self.load()?;
        let mut project = base.clone();
        let ix = find_env(&project, args.req_str("name")?)?;
        project.active_env = ix;
        self.save(&base, &project)?;
        let env = &project.environments[ix];
        Ok(json!({"active": env.name, "id": env.id, "production": env.production}).into())
    }

    fn create_environment(&self, args: &Args) -> ToolResult {
        let base = self.load()?;
        let mut project = base.clone();
        let name = args.req_str("name")?.trim();
        if project.environments.iter().any(|e| e.name.eq_ignore_ascii_case(name)) {
            return Err(format!("Environment '{name}' already exists; use set_variable to change its variables"));
        }
        let production = args.flag("production")?;
        let color = match args.str("color")? {
            None => if production { EnvColor::Red } else { EnvColor::Green },
            Some(c) => serde_json::from_value(json!(c.trim().to_lowercase()))
                .map_err(|_| format!("Unknown color '{c}'; use green, red, amber, blue or purple"))?,
        };
        let mut env = Environment::new(name, color);
        env.production = production;
        if let Some(vars) = args.get("variables") {
            env.vars = args::key_values(vars, "variables")?;
            env.vars.iter_mut().for_each(|v| v.file = false);
        }
        let (id, variable_count) = (env.id.clone(), env.vars.len());
        project.environments.push(env);
        let ix = project.environments.len() - 1;
        if args.flag("activate")? || project.environments.get(project.active_env).is_none() {
            project.active_env = ix;
        }
        let active = project.active_env == ix;
        self.save(&base, &project)?;
        Ok(json!({
            "id": id,
            "name": name,
            "production": production,
            "color": color,
            "variable_count": variable_count,
            "active": active,
        })
        .into())
    }

    /// Variables of the scope named by `scope`/`name`.
    fn scope_vars<'p>(project: &'p mut Project, args: &Args) -> Result<(String, String, &'p mut Vec<KeyValue>), String> {
        match args.req_str("scope")? {
            "environment" | "env" => {
                let ix = match args.str("name")? {
                    Some(n) => find_env(project, n)?,
                    None if project.environments.get(project.active_env).is_some() => project.active_env,
                    None => return Err("There is no active environment; pass name, or use scope \"collection\"".into()),
                };
                let env = &mut project.environments[ix];
                Ok((env.id.clone(), env.name.clone(), &mut env.vars))
            }
            "collection" => {
                let name = args.str("name")?.ok_or("'name' (collection id or name) is required for scope \"collection\"")?;
                let ix = find_collection(project, name)?;
                let c = &mut project.collections[ix];
                Ok((c.id.clone(), c.name.clone(), &mut c.vars))
            }
            other => Err(format!("Unknown scope '{other}'; use \"environment\" or \"collection\"")),
        }
    }

    fn get_variables(&self, args: &Args) -> ToolResult {
        let mut project = self.load()?;
        let reveal = self.reveal(args)?;
        let (id, name, vars) = Self::scope_vars(&mut project, args)?;
        let variables = output::variables(vars, reveal);
        Ok(json!({"scope": args.req_str("scope")?, "id": id, "name": name, "variables": variables}).into())
    }

    fn set_variable(&self, args: &Args, set: bool) -> ToolResult {
        let base = self.load()?;
        let mut project = base.clone();
        let key = args.req_str("key")?.to_string();
        let value = if set {
            match args.get("value") {
                Some(Value::String(s)) => Some(s.clone()),
                Some(other) => Some(other.to_string()),
                None => return Err("'value' is required".into()),
            }
        } else {
            None
        };
        let enabled = args.bool("enabled")?.unwrap_or(true);
        let (id, name, vars) = Self::scope_vars(&mut project, args)?;
        let existed = vars.iter().any(|v| v.key == key);
        match &value {
            Some(value) => match vars.iter_mut().find(|v| v.key == key) {
                Some(v) => {
                    v.value = value.clone();
                    v.enabled = enabled;
                }
                None => vars.push(KeyValue::new(key.clone(), value.clone()).enabled(enabled)),
            },
            None if !existed => return Err(format!("Variable '{key}' does not exist in {name}")),
            None => vars.retain(|v| v.key != key),
        }
        self.save(&base, &project)?;
        Ok(json!({
            "scope": args.req_str("scope")?,
            "id": id,
            "name": name,
            "key": key,
            "action": if !set { "removed" } else if existed { "updated" } else { "created" },
        })
        .into())
    }

    fn import_spec(&self, args: &Args) -> ToolResult {
        let path = PathBuf::from(args.req_str("path")?);
        let path = if path.is_relative() {
            std::env::current_dir().map(|d| d.join(&path)).unwrap_or(path)
        } else {
            path
        };
        let imported = import::import_file(&path)?;
        let base = self.load()?;
        let mut project = base.clone();
        let c = imported.collection;
        let summary = json!({
            "collection_id": c.id,
            "collection": c.name,
            "format": imported.format,
            "request_count": c.requests.len(),
            "folders": c.folders,
            "variables": output::variables(&c.vars, false),
            "linked_file": c.source.as_ref().map(|p| p.display().to_string()),
            "warnings": imported.warnings,
        });
        project.collections.push(c);
        self.save(&base, &project)?;
        Ok(summary.into())
    }

    fn export_collection(&self, args: &Args) -> ToolResult {
        let project = self.load()?;
        let mut collection = project.collections[find_collection(&project, args.req_str("collection")?)?].clone();
        let format = match args.str("format")? {
            Some(id) => ExportFormat::from_id(id)
                .ok_or_else(|| format!("Unknown format '{id}': use rivet, postman or openapi"))?,
            None => ExportFormat::Rivet,
        };
        let path = args.str("path")?.map(PathBuf::from);
        // Same rule for inline content and files (a file the model writes it
        // can also read): secrets stay masked unless Settings → MCP allows it.
        if !self.reveal(args)? {
            secrets::mask_key_values(&mut collection.vars);
            for request in &mut collection.requests {
                secrets::mask_request(request);
            }
        }
        let exported = export::export(&collection, format)?;
        let mut value = json!({
            "collection": collection.name,
            "format": format.id(),
            "request_count": collection.requests.len(),
            "warnings": exported.warnings,
        });
        match path {
            Some(path) => {
                if path.is_relative() {
                    return Err("path must be absolute".into());
                }
                std::fs::write(&path, &exported.text).map_err(|e| format!("{}: {e}", path.display()))?;
                value["path"] = json!(path.display().to_string());
                value["bytes"] = json!(exported.text.len());
            }
            None => value["content"] = json!(exported.text),
        }
        Ok(value.into())
    }

    fn run_collection(&self, args: &Args) -> ToolResult {
        let project = self.load()?;
        let reveal = self.reveal(args)?;
        let opts = CollectionRunOptions {
            collection: args.req_str("collection")?.to_string(),
            folder: args.str("folder")?.map(String::from),
            environment: args.str("environment")?.map(String::from),
            stop_on_failure: args.flag("stop_on_failure")?,
            delay_ms: args.u64("delay_ms")?.unwrap_or(0).min(60_000),
        };
        // Up-front checks give readable errors and the production guard.
        let ci = find_collection(&project, &opts.collection)?;
        let env = match &opts.environment {
            Some(name) => Some(&project.environments[find_env(&project, name)?]),
            None => project_ops::active_env(&project),
        };
        if let Some(env) = env.filter(|e| e.production) {
            let destructive: Vec<String> = project.collections[ci]
                .requests
                .iter()
                .filter(|r| opts.folder.as_deref().is_none_or(|f| project_ops::in_folder(r, f)))
                .filter(|r| is_destructive(r.method))
                .map(|r| format!("{} {}", r.method.full_label(), r.name))
                .collect();
            if !destructive.is_empty() && !args.flag("confirm_production")? {
                return Err(production_refusal(env, &format!("This run contains {}", destructive.join(", "))));
            }
        }

        let report = collection_run::run_collection(&project, opts, &CancelToken::new(), Some(&self.sessions));
        if let Some(e) = &report.error {
            return Err(e.clone());
        }
        let persist = !self.policy.read_only;
        if persist {
            let base = self.load()?;
            let mut fresh = base.clone();
            project_ops::apply_var_changes(
                &mut fresh,
                Some(&report.collection_id),
                report.environment_id.as_deref(),
                &report.env_changes,
                &report.collection_changes,
            );
            let env_name = report.environment.clone().unwrap_or_default();
            for item in &report.items {
                if let Some(sent) = &item.sent {
                    store::push_history(&mut fresh, project_ops::history_entry(&env_name, sent.clone(), item.status, item.duration_ms));
                }
            }
            self.save(&base, &fresh)?;
        }

        let mut value = serde_json::to_value(&report).map_err(|e| e.to_string())?;
        value["env_changes"] = output::var_changes(&report.env_changes, reveal);
        value["collection_changes"] = output::var_changes(&report.collection_changes, reveal);
        if let Some(items) = value["items"].as_array_mut() {
            for item in items {
                if let Some(url) = item["url"].as_str() {
                    item["url"] = json!(output::mask_url(url, reveal));
                }
            }
        }
        if !persist {
            value["persisted"] = json!(false);
            value["note"] = json!(READ_ONLY_NOTE);
        }
        // Failed tests are results, not tool errors: the report carries them.
        Ok(value.into())
    }
}

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

/// One-line description of a successful call for the activity log. URLs are
/// masked; values of variables are never included.
fn summarize(name: &str, v: &Value) -> String {
    let count = |key: &str| v[key].as_array().map_or(0, Vec::len);
    match name {
        "send_request" => {
            let target = format!("{} {}", text(&v["request"]["method"]), output::mask_url(text(&v["request"]["url"]), false));
            match (v["response"]["status"].as_u64(), v["error"].as_str()) {
                (Some(status), _) => format!("{target} → {status}"),
                (None, Some(error)) => format!("{target} → {error}"),
                _ => target,
            }
        }
        "run_collection" => {
            let t = &v["totals"];
            format!(
                "{}: запросов {}, ошибок {}, тесты {}/{}",
                text(&v["collection"]),
                t["requests"].as_u64().unwrap_or(0),
                t["failed_requests"].as_u64().unwrap_or(0),
                t["passed_tests"].as_u64().unwrap_or(0),
                t["passed_tests"].as_u64().unwrap_or(0) + t["failed_tests"].as_u64().unwrap_or(0),
            )
        }
        "list_collections" => format!("коллекций: {}", count("collections")),
        "list_requests" => format!("запросов: {}", v["count"].as_u64().unwrap_or(0)),
        "get_request" => format!(
            "{} / {} {}",
            text(&v["collection"]),
            text(&v["request"]["method"]),
            text(&v["request"]["name"])
        ),
        "save_request" => format!(
            "{} · {} ({})",
            text(&v["collection"]),
            text(&v["id"]),
            if v["created"].as_bool() == Some(true) { "создан" } else { "обновлён" }
        ),
        "get_history" => format!("записей: {}", count("entries")),
        "list_environments" => format!("окружений: {}", count("environments")),
        "set_active_environment" => format!("активно: {}", text(&v["active"])),
        "create_environment" => format!(
            "{}{} (переменных: {})",
            text(&v["name"]),
            if v["production"].as_bool() == Some(true) { ", production" } else { "" },
            v["variable_count"].as_u64().unwrap_or(0)
        ),
        "get_variables" => format!("{} {}: переменных {}", text(&v["scope"]), text(&v["name"]), count("variables")),
        "set_variable" | "unset_variable" => format!("{} {}: {} {}", text(&v["scope"]), text(&v["name"]), text(&v["key"]), text(&v["action"])),
        "import_spec" => format!("{} (запросов: {})", text(&v["collection"]), v["request_count"].as_u64().unwrap_or(0)),
        _ => String::new(),
    }
}
