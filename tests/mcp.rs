//! `rivet mcp` end to end: spawns the binary on a temp project and speaks
//! JSON-RPC over its stdio, against a tiny local HTTP server.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use rivet::core::secrets::MASK;
use rivet::core::mcp_activity;
use rivet::core::{Collection, EnvColor, Environment, KeyValue, McpSettings, Method, Project, RequestSpec, store};
use serde_json::{Value, json};

/// Answers `/login` with `{"token":"tok-<n>"}`, everything else with an echo
/// `{"method","path","auth","body"}`.
fn server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for (n, stream) in listener.incoming().enumerate() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).ok();
            let mut parts = line.split_whitespace();
            let method = parts.next().unwrap_or("").to_string();
            let target = parts.next().unwrap_or("").to_string();
            let (mut len, mut auth) = (0usize, String::new());
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
                    break;
                }
                if let Some((k, v)) = h.trim().split_once(':') {
                    if k.eq_ignore_ascii_case("content-length") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    if k.eq_ignore_ascii_case("authorization") {
                        auth = v.trim().to_string();
                    }
                }
            }
            let mut body = vec![0; len];
            reader.read_exact(&mut body).ok();
            let json = if target == "/login" {
                json!({"token": format!("tok-{n}")})
            } else {
                json!({"method": method, "path": target, "auth": auth, "body": String::from_utf8_lossy(&body)})
            }
            .to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Session-Token: srv-secret\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                json.len()
            );
        }
    });
    format!("http://{addr}")
}

struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Mcp {
    fn start(project: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rivet"))
            .args(["mcp", "--project"])
            .arg(project)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn rivet mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut mcp = Mcp { child, stdin, stdout, next_id: 1 };
        let init = mcp.rpc(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}),
        );
        assert_eq!(init["result"]["serverInfo"]["name"], "rivet");
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        assert!(init["result"]["capabilities"]["tools"].is_object());
        mcp.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        mcp
    }

    fn send(&mut self, message: Value) {
        writeln!(self.stdin, "{message}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn rpc(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad reply {line:?}: {e}"));
        assert_eq!(reply["id"], id, "{reply}");
        reply
    }

    /// Calls a tool; returns (parsed JSON or raw text, isError).
    fn tool(&mut self, name: &str, arguments: Value) -> (Value, bool) {
        let reply = self.rpc("tools/call", json!({"name": name, "arguments": arguments}));
        let result = &reply["result"];
        assert!(result.is_object(), "{reply}");
        let text = result["content"][0]["text"].as_str().unwrap().to_string();
        let value = serde_json::from_str(&text).unwrap_or(Value::String(text));
        (value, result["isError"].as_bool().unwrap_or(false))
    }

    fn ok(&mut self, name: &str, arguments: Value) -> Value {
        let (value, is_error) = self.tool(name, arguments);
        assert!(!is_error, "{name} failed: {value}");
        value
    }

    fn err(&mut self, name: &str, arguments: Value) -> String {
        let (value, is_error) = self.tool(name, arguments);
        assert!(is_error, "{name} should fail: {value}");
        value.as_str().map(String::from).unwrap_or_else(|| value.to_string())
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Edits `settings.mcp` in the project file (the server re-reads it per call).
fn set_mcp(path: &Path, edit: impl FnOnce(&mut McpSettings)) {
    let mut project = store::load(path).unwrap();
    edit(&mut project.settings.mcp);
    store::save(path, &project).unwrap();
}

fn seed(dir: &Path, base: &str) -> PathBuf {
    let mut p: Project = store::seed_project();
    let mut local = Environment::new("Local", EnvColor::Green);
    local.vars = vec![KeyValue::new("baseUrl", base), KeyValue::new("apiToken", "env-secret")];
    let mut prod = Environment::new("Prod", EnvColor::Red);
    prod.production = true;
    prod.vars = vec![KeyValue::new("baseUrl", base)];
    p.environments = vec![local, prod];

    let mut c = Collection::new("Shop");
    c.folders = vec!["Auth".into()];
    let mut login = RequestSpec::new("Login", Method::Post, "{{baseUrl}}/login");
    login.folder = Some("Auth".into());
    login.scripts.post_response = "pm.collectionVariables.set('sessionToken', pm.response.json().token);\npm.test('has token', () => pm.expect(pm.response.json().token).to.be.a('string'));".into();
    let mut me = RequestSpec::new("Me", Method::Get, "{{baseUrl}}/me");
    me.headers = vec![KeyValue::new("Authorization", "Bearer {{sessionToken}}")];
    me.scripts.post_response =
        "pm.test('sent token', () => pm.expect(pm.response.json().auth).to.equal('Bearer ' + pm.collectionVariables.get('sessionToken')));".into();
    for mut r in [login, me] {
        r.collection_id = Some(c.id.clone());
        c.requests.push(r);
    }
    p.collections.push(c);
    p.collections.push(Collection::new("Empty"));
    let path = dir.join("project.json");
    store::save(&path, &p).unwrap();
    path
}

#[test]
fn protocol_basics_and_tool_list() {
    let dir = tempfile::tempdir().unwrap();
    let path = seed(dir.path(), "http://127.0.0.1:9");
    let mut mcp = Mcp::start(&path);

    assert_eq!(mcp.rpc("ping", json!({}))["result"], json!({}));
    let tools = mcp.rpc("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for expected in [
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
    ] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }
    for t in tools["result"]["tools"].as_array().unwrap() {
        assert_eq!(t["inputSchema"]["type"], "object", "{t}");
        assert!(t["description"].as_str().unwrap().len() > 20);
    }
    assert_eq!(mcp.rpc("nope/method", json!({}))["error"]["code"], -32601);
    assert_eq!(mcp.rpc("tools/call", json!({"name": "nope"}))["error"]["code"], -32602);

    let cols = mcp.ok("list_collections", json!({}));
    let shop = &cols["collections"][0];
    assert_eq!(shop["name"], "Shop");
    assert_eq!(shop["request_count"], 2);
    assert_eq!(shop["folders"][0], json!({"path": "Auth", "requests": 1}));
    assert_eq!(cols["collections"][1]["request_count"], 0);

    let listed = mcp.ok("list_requests", json!({"query": "me"}));
    assert_eq!(listed["count"], 1);
    assert_eq!(listed["requests"][0]["name"], "Me");
    let listed = mcp.ok("list_requests", json!({"collection": "shop", "folder": "Auth"}));
    assert_eq!(listed["requests"][0]["name"], "Login");
    assert!(mcp.err("list_requests", json!({"collection": "Nope"})).contains("Shop"));
}

#[test]
fn save_get_round_trip_masks_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let path = seed(dir.path(), "http://127.0.0.1:9");
    let mut mcp = Mcp::start(&path);

    let saved = mcp.ok(
        "save_request",
        json!({
            "collection": "Shop", "folder": "Orders/Admin", "method": "POST",
            "url": "{{baseUrl}}/orders?api_key=k123",
            "headers": {"X-Api-Key": "hdr-secret", "Accept": "application/json"},
            "body": {"item": 1},
            "auth": {"type": "bearer", "token": "bearer-secret"}
        }),
    );
    let id = saved["id"].as_str().unwrap().to_string();
    assert_eq!(saved["created"], true);

    let got = mcp.ok("get_request", json!({"id": id}));
    let r = &got["request"];
    assert_eq!(r["name"], "orders");
    assert_eq!(r["method"], "POST");
    assert_eq!(r["folder"], "Orders / Admin");
    assert_eq!(r["auth"]["token"], MASK);
    assert_eq!(r["headers"][0]["value"], MASK);
    assert_eq!(r["headers"][1]["value"], "application/json");
    assert!(r["url"].as_str().unwrap().contains(&format!("api_key={MASK}")));
    assert_eq!(r["body"]["kind"], "json");

    // reveal_secrets is ignored until Settings → MCP allows it.
    let still_masked = mcp.ok("get_request", json!({"id": id, "reveal_secrets": true}));
    assert_eq!(still_masked["request"]["auth"]["token"], MASK);
    set_mcp(&path, |m| m.reveal_secrets = true);
    let revealed = mcp.ok("get_request", json!({"id": id, "reveal_secrets": true}));
    assert_eq!(revealed["request"]["auth"]["token"], "bearer-secret");

    // Sending the masked object back keeps the stored secrets.
    let mut update = r.clone();
    update["name"] = json!("Create order");
    update["url"] = json!("{{baseUrl}}/orders?api_key=k123");
    let again = mcp.ok("save_request", update);
    assert_eq!(again["id"], id.as_str());
    assert_eq!(again["created"], false);

    let stored = store::load(&path).unwrap();
    let shop = &stored.collections[0];
    let req = shop.requests.iter().find(|r| r.id == id).unwrap();
    assert_eq!(req.name, "Create order");
    assert_eq!(req.headers[0].value, "hdr-secret");
    assert_eq!(req.auth, rivet::core::Auth::Bearer { token: "bearer-secret".into() });
    assert!(shop.folders.contains(&"Orders".to_string()) && shop.folders.contains(&"Orders / Admin".to_string()));

    let by_path = mcp.ok("get_request", json!({"id": "Shop / Orders / Admin / Create order"}));
    assert_eq!(by_path["request"]["id"], id.as_str());
    assert!(mcp.err("save_request", json!({"url": "http://x"})).contains("collection"));
    assert!(mcp.err("save_request", json!({"id": "missing", "name": "x"})).contains("missing"));

    let created = mcp.ok("save_request", json!({"collection": "Brand new", "url": "http://x/ping"}));
    assert_eq!(created["collection_created"], true);
}

#[test]
fn send_request_persists_script_variables_and_history() {
    let base = server();
    let dir = tempfile::tempdir().unwrap();
    let path = seed(dir.path(), &base);
    let mut mcp = Mcp::start(&path);

    let sent = mcp.ok("send_request", json!({"request_id": "Login"}));
    let response = &sent["response"];
    assert_eq!(response["status"], 200);
    assert_eq!(response["http_version"], "HTTP/1.1");
    assert!(response["body"].as_str().unwrap().contains("tok-"));
    assert_eq!(response["body_truncated"], false);
    assert!(response["timings_ms"]["total"].as_f64().unwrap() >= 0.0);
    let session_header = response["headers"].as_array().unwrap().iter().find(|h| h["name"].as_str().unwrap().eq_ignore_ascii_case("x-session-token")).unwrap();
    assert_eq!(session_header["value"], MASK);
    assert_eq!(sent["request"]["url"], format!("{base}/login"));
    assert_eq!(sent["scripts"]["tests"][0]["passed"], true);
    assert_eq!(sent["variable_changes"]["collection"][0]["key"], "sessionToken");
    assert_eq!(sent["variable_changes"]["collection"][0]["set"], MASK);

    let stored = store::load(&path).unwrap();
    let var = stored.collections[0].vars.iter().find(|v| v.key == "sessionToken").expect("persisted");
    assert!(var.value.starts_with("tok-"));
    assert_eq!(stored.history.len(), 1);
    assert_eq!(stored.history[0].status, Some(200));
    assert_eq!(stored.history[0].env, "Local");

    // The next request picks the persisted variable up.
    let me = mcp.ok("send_request", json!({"request_id": "Me"}));
    assert_eq!(me["scripts"]["tests"][0]["passed"], true, "{me}");

    // Ad-hoc request with truncation.
    let adhoc = mcp.ok("send_request", json!({"method": "PUT", "url": format!("{base}/echo"), "body": "x".repeat(100), "max_body_chars": 10}));
    assert_eq!(adhoc["response"]["body_truncated"], true);
    assert_eq!(adhoc["response"]["body"].as_str().unwrap().chars().count(), 10);

    let history = mcp.ok("get_history", json!({"limit": 2}));
    assert_eq!(history["total"], 3);
    assert_eq!(history["entries"].as_array().unwrap().len(), 2);
    assert_eq!(history["entries"][0]["method"], "PUT");

    // Network errors are tool errors with the reason.
    let (failed, is_error) = mcp.tool("send_request", json!({"url": "http://127.0.0.1:9/x"}));
    assert!(is_error);
    assert!(failed["error"].as_str().unwrap().len() > 3, "{failed}");
}

#[test]
fn production_requires_confirmation() {
    let base = server();
    let dir = tempfile::tempdir().unwrap();
    let path = seed(dir.path(), &base);
    let mut mcp = Mcp::start(&path);

    let refusal = mcp.err("send_request", json!({"request_id": "Login", "environment": "Prod"}));
    assert!(refusal.contains("production") && refusal.contains("confirm_production"), "{refusal}");
    assert!(store::load(&path).unwrap().history.is_empty());
    // GET is fine without confirmation; POST with it.
    mcp.ok("send_request", json!({"request_id": "Me", "environment": "Prod"}));
    let confirmed = mcp.ok("send_request", json!({"request_id": "Login", "environment": "Prod", "confirm_production": true}));
    assert_eq!(confirmed["response"]["status"], 200);

    mcp.ok("set_active_environment", json!({"name": "prod"}));
    assert_eq!(store::load(&path).unwrap().active_env, 1);
    let refusal = mcp.err("run_collection", json!({"collection": "Shop"}));
    assert!(refusal.contains("POST Login"), "{refusal}");
}

#[test]
fn variables_set_get_unset() {
    let dir = tempfile::tempdir().unwrap();
    let path = seed(dir.path(), "http://127.0.0.1:9");
    let mut mcp = Mcp::start(&path);

    let envs = mcp.ok("list_environments", json!({}));
    assert_eq!(envs["active"], "Local");
    assert_eq!(envs["environments"][1]["production"], true);

    let vars = mcp.ok("get_variables", json!({"scope": "environment"}));
    assert_eq!(vars["name"], "Local");
    let token = vars["variables"].as_array().unwrap().iter().find(|v| v["key"] == "apiToken").unwrap();
    assert_eq!((token["value"].as_str(), token["secret"].as_bool()), (Some(MASK), Some(true)));

    let set = mcp.ok("set_variable", json!({"scope": "collection", "name": "Shop", "key": "page", "value": "2"}));
    assert_eq!(set["action"], "created");
    mcp.ok("set_variable", json!({"scope": "environment", "name": "Prod", "key": "region", "value": "eu"}));
    let stored = store::load(&path).unwrap();
    assert_eq!(stored.collections[0].vars[0].value, "2");
    assert_eq!(stored.environments[1].var("region"), Some("eu"));

    let col = mcp.ok("get_variables", json!({"scope": "collection", "name": "Shop"}));
    assert_eq!(col["variables"][0], json!({"key": "page", "value": "2", "enabled": true}));
    assert_eq!(mcp.ok("set_variable", json!({"scope": "collection", "name": "Shop", "key": "page", "value": "3"}))["action"], "updated");
    mcp.ok("unset_variable", json!({"scope": "collection", "name": "Shop", "key": "page"}));
    assert!(store::load(&path).unwrap().collections[0].vars.is_empty());
    assert!(mcp.err("unset_variable", json!({"scope": "collection", "name": "Shop", "key": "page"})).contains("does not exist"));
    assert!(mcp.err("get_variables", json!({"scope": "collection"})).contains("required"));
}

#[test]
fn create_environment_with_variables_and_production_flag() {
    let dir = tempfile::tempdir().unwrap();
    // No project file yet: the first environment becomes active.
    let path = dir.path().join("project.json");
    let mut mcp = Mcp::start(&path);

    let made = mcp.ok("create_environment", json!({"name": "Dev", "variables": {"baseUrl": "http://localhost:3000", "apiToken": "sk-1"}}));
    assert_eq!((made["active"].as_bool(), made["color"].as_str(), made["variable_count"].as_u64()), (Some(true), Some("green"), Some(2)));
    let made = mcp.ok("create_environment", json!({
        "name": "Prod", "production": true,
        "variables": [{"key": "baseUrl", "value": "https://api.example.com"}, {"key": "debug", "value": "1", "enabled": false}]
    }));
    assert_eq!((made["active"].as_bool(), made["color"].as_str()), (Some(false), Some("red")));
    mcp.ok("create_environment", json!({"name": "Stage", "color": "Blue", "activate": true}));

    let stored = store::load(&path).unwrap();
    let names: Vec<&str> = stored.environments.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["Dev", "Prod", "Stage"]);
    assert_eq!(stored.active_env, 2);
    assert_eq!(stored.environments[0].var("apiToken"), Some("sk-1"));
    let prod = &stored.environments[1];
    assert!(prod.production);
    assert_eq!(prod.var("debug"), None, "disabled variable");
    assert_eq!(stored.environments[2].color, rivet::core::EnvColor::Blue);

    let vars = mcp.ok("get_variables", json!({"scope": "environment", "name": "Dev"}));
    assert!(!vars.to_string().contains("sk-1"), "secret masked: {vars}");
    assert!(mcp.err("create_environment", json!({"name": "dev"})).contains("already exists"));
    assert!(mcp.err("create_environment", json!({"name": "X", "color": "pink"})).contains("Unknown color"));
    assert!(mcp.err("create_environment", json!({"name": "  "})).contains("required"));
    assert!(mcp.err("send_request", json!({"method": "DELETE", "url": "{{baseUrl}}/x", "environment": "Prod"})).contains("production"));
}

#[test]
fn run_collection_chains_requests() {
    let base = server();
    let dir = tempfile::tempdir().unwrap();
    let path = seed(dir.path(), &base);
    let mut mcp = Mcp::start(&path);

    let report = mcp.ok("run_collection", json!({"collection": "Shop", "stop_on_failure": true}));
    let items = report["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{report}");
    assert_eq!(items[0]["name"], "Login");
    assert_eq!(items[1]["tests"][0]["passed"], true, "{report}");
    assert_eq!(report["totals"]["passed_tests"], 2);
    assert_eq!(report["totals"]["failed_requests"], 0);
    assert_eq!(report["environment"], "Local");

    let stored = store::load(&path).unwrap();
    assert!(stored.collections[0].vars.iter().any(|v| v.key == "sessionToken" && v.value.starts_with("tok-")));
    assert_eq!(stored.history.len(), 2);

    let folder = mcp.ok("run_collection", json!({"collection": "Shop", "folder": "Auth"}));
    assert_eq!(folder["items"].as_array().unwrap().len(), 1);
}

#[test]
fn import_spec_creates_linked_collection() {
    let dir = tempfile::tempdir().unwrap();
    let path = seed(dir.path(), "http://127.0.0.1:9");
    let mut mcp = Mcp::start(&path);
    let spec = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/shop-api.yaml");

    let imported = mcp.ok("import_spec", json!({"path": spec}));
    assert_eq!(imported["collection"], "Shop API");
    assert_eq!(imported["format"], "openapi");
    assert!(imported["request_count"].as_u64().unwrap() > 3);
    let stored = store::load(&path).unwrap();
    let c = stored.collections.iter().find(|c| c.name == "Shop API").unwrap();
    assert_eq!(c.source.as_deref(), Some(spec.as_path()));
    assert!(c.requests.iter().all(|r| r.spec_ref.is_some() && r.collection_id.as_deref() == Some(c.id.as_str())));

    let listed = mcp.ok("list_requests", json!({"collection": "Shop API"}));
    assert!(listed["requests"][0]["spec_operation"].is_string());
    assert!(mcp.err("import_spec", json!({"path": dir.path().join("missing.yaml")})).contains("missing.yaml"));
}

#[test]
fn disabled_mcp_refuses_tool_calls() {
    let dir = tempfile::tempdir().unwrap();
    let path = seed(dir.path(), "http://127.0.0.1:9");
    set_mcp(&path, |m| m.enabled = false);
    // initialize (in start) and tools/list still work.
    let mut mcp = Mcp::start(&path);
    assert!(!mcp.rpc("tools/list", json!({}))["result"]["tools"].as_array().unwrap().is_empty());
    let err = mcp.err("list_collections", json!({}));
    assert_eq!(err, rivet::mcp::DISABLED_MESSAGE);
    assert!(err.contains("Настройки → MCP"));
    // Turned back on in the app: the next call works without a restart.
    set_mcp(&path, |m| m.enabled = true);
    mcp.ok("list_collections", json!({}));
}

#[test]
fn read_only_refuses_mutations_and_sends_without_persisting() {
    let base = server();
    let dir = tempfile::tempdir().unwrap();
    let path = seed(dir.path(), &base);
    set_mcp(&path, |m| m.read_only = true);
    let before = std::fs::read_to_string(&path).unwrap();
    let mut mcp = Mcp::start(&path);

    for (tool, args) in [
        ("save_request", json!({"collection": "Shop", "url": "http://x/ping"})),
        ("set_variable", json!({"scope": "collection", "name": "Shop", "key": "k", "value": "v"})),
        ("unset_variable", json!({"scope": "collection", "name": "Shop", "key": "k"})),
        ("set_active_environment", json!({"name": "Prod"})),
        ("create_environment", json!({"name": "Blocked"})),
        ("import_spec", json!({"path": "/nope.yaml"})),
    ] {
        let err = mcp.err(tool, args);
        assert!(err.contains("Только чтение") && err.contains(tool), "{err}");
    }
    mcp.ok("list_collections", json!({}));

    let sent = mcp.ok("send_request", json!({"request_id": "Login"}));
    assert_eq!(sent["response"]["status"], 200);
    assert_eq!(sent["persisted"], false);
    assert_eq!(sent["variable_changes"]["collection"][0]["key"], "sessionToken");
    let run = mcp.ok("run_collection", json!({"collection": "Shop"}));
    assert_eq!(run["persisted"], false);
    assert_eq!(run["items"].as_array().unwrap().len(), 2);
    // No history, no variables: the file is byte-for-byte unchanged.
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
}

#[test]
fn sending_can_be_forbidden() {
    let base = server();
    let dir = tempfile::tempdir().unwrap();
    let path = seed(dir.path(), &base);
    set_mcp(&path, |m| m.allow_send = false);
    let mut mcp = Mcp::start(&path);
    assert!(mcp.err("send_request", json!({"request_id": "Login"})).contains("Разрешить отправку запросов"));
    assert!(mcp.err("run_collection", json!({"collection": "Shop"})).contains("run_collection"));
    // Editing still works.
    mcp.ok("set_variable", json!({"scope": "collection", "name": "Shop", "key": "page", "value": "1"}));
    assert!(store::load(&path).unwrap().history.is_empty());
}

#[test]
fn reveal_secrets_needs_permission() {
    let dir = tempfile::tempdir().unwrap();
    let path = seed(dir.path(), "http://127.0.0.1:9");
    let mut mcp = Mcp::start(&path);
    let token = |vars: &Value| vars["variables"].as_array().unwrap().iter().find(|v| v["key"] == "apiToken").unwrap()["value"].clone();
    let masked = mcp.ok("get_variables", json!({"scope": "environment", "reveal_secrets": true}));
    assert_eq!(token(&masked), MASK);
    set_mcp(&path, |m| m.reveal_secrets = true);
    let shown = mcp.ok("get_variables", json!({"scope": "environment", "reveal_secrets": true}));
    assert_eq!(token(&shown), "env-secret");
    let default = mcp.ok("get_variables", json!({"scope": "environment"}));
    assert_eq!(token(&default), MASK);
}

#[test]
fn activity_log_records_calls() {
    let base = server();
    let dir = tempfile::tempdir().unwrap();
    let path = seed(dir.path(), &base);
    let log = mcp_activity::activity_path(&path);
    let mut mcp = Mcp::start(&path);
    mcp.ok("list_collections", json!({}));
    mcp.ok("send_request", json!({"method": "GET", "url": format!("{base}/items?api_key=k123")}));
    mcp.err("get_request", json!({"id": "missing"}));
    set_mcp(&path, |m| m.enabled = false);
    mcp.err("list_environments", json!({}));

    let entries = mcp_activity::read(&log, 10);
    assert_eq!(entries.len(), 4, "{entries:?}");
    assert_eq!(entries[0].tool, "list_environments");
    assert!(!entries[0].ok);
    assert!(entries[0].summary.contains("отключён"));
    assert!(!entries[1].ok);
    let send = &entries[2];
    assert_eq!(send.tool, "send_request");
    assert!(send.ok);
    assert_eq!(send.summary, format!("GET {base}/items?api_key={MASK} → 200"));
    assert!(!send.summary.contains("k123"));
    assert_eq!(send.client.as_deref(), Some("test"));
    assert_eq!(entries[3].summary, "коллекций: 2");
    assert!(entries[3].at > 0);
}

#[test]
fn export_collection_writes_files_and_masks_inline_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let path = seed(dir.path(), "http://127.0.0.1:9");
    let mut project = store::load(&path).unwrap();
    project.collections[0].requests[0].auth = rivet::core::Auth::Bearer { token: "super-secret-token".into() };
    store::save(&path, &project).unwrap();
    let mut mcp = Mcp::start(&path);

    // Inline: secrets masked.
    let inline = mcp.ok("export_collection", json!({"collection": "Shop", "format": "postman"}));
    let content = inline["content"].as_str().unwrap();
    assert!(!content.contains("super-secret-token"));
    assert!(content.contains(MASK));
    assert!(content.contains("v2.1.0"));
    // reveal_secrets needs Settings → MCP permission — for files too.
    let file = dir.path().join("shop.rivet.json");
    mcp.ok("export_collection", json!({"collection": "Shop", "path": file, "reveal_secrets": true}));
    assert!(!std::fs::read_to_string(&file).unwrap().contains("super-secret-token"));
    set_mcp(&path, |m| m.reveal_secrets = true);
    let revealed = mcp.ok("export_collection", json!({"collection": "Shop", "format": "postman", "reveal_secrets": true}));
    assert!(revealed["content"].as_str().unwrap().contains("super-secret-token"));

    // To a file: re-importable (Rivet format).
    let written = mcp.ok("export_collection", json!({"collection": "Shop", "path": file, "reveal_secrets": true}));
    assert_eq!(written["format"], "rivet");
    assert_eq!(written["request_count"], 2);
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("super-secret-token"));

    let yaml = mcp.ok("export_collection", json!({"collection": "Shop", "format": "openapi"}));
    assert!(yaml["content"].as_str().unwrap().contains("openapi: 3.0.3"));
    assert!(mcp.err("export_collection", json!({"collection": "Shop", "format": "har"})).contains("Unknown format"));
    assert!(mcp.err("export_collection", json!({"collection": "Shop", "path": "rel.json"})).contains("absolute"));
    let imported = mcp.ok("import_spec", json!({"path": file}));
    assert_eq!(imported["format"], "rivet");
    assert_eq!(imported["request_count"], 2);
}
