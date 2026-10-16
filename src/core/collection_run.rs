//! Collection runner: sends the requests of a collection (or one folder) in
//! order through `core::runner`, carrying variables set by scripts from one
//! request to the next. UI-free; used by the MCP server, reusable by the app.

use std::time::{Duration, Instant};

use serde::Serialize;

use super::http::{CancelToken, HttpSessions};
use super::project_ops::{self, find_collection, find_env, in_folder};
use super::runner;
use super::types::{Environment, Project, RequestSpec, TestResult, VarChange};

#[derive(Clone, Debug, Default)]
pub struct CollectionRunOptions {
    /// Collection id or name.
    pub collection: String,
    /// Only requests in this folder (and its subfolders).
    pub folder: Option<String>,
    /// Environment id or name instead of the active one.
    pub environment: Option<String>,
    /// Stop after the first request that errors or fails a test.
    pub stop_on_failure: bool,
    /// Pause between requests.
    pub delay_ms: u64,
}

/// One executed (or skipped) request.
#[derive(Clone, Debug, Serialize)]
pub struct CollectionRunItem {
    pub request_id: String,
    pub name: String,
    pub method: String,
    /// URL after variable resolution (the template when resolution failed).
    pub url: String,
    pub status: Option<u16>,
    pub duration_ms: Option<u64>,
    pub tests: Vec<TestResult>,
    pub error: Option<String>,
    pub script_logs: usize,
    /// Set for requests that were not sent (WebSocket requests).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    /// The request as sent (for history); not part of the report JSON.
    #[serde(skip)]
    pub sent: Option<RequestSpec>,
}

impl CollectionRunItem {
    pub fn failed(&self) -> bool {
        self.error.is_some() || self.tests.iter().any(|t| !t.passed)
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct CollectionRunTotals {
    pub requests: usize,
    pub passed_tests: usize,
    pub failed_tests: usize,
    pub failed_requests: usize,
    pub skipped: usize,
    pub total_ms: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct CollectionRunReport {
    pub collection_id: String,
    pub collection: String,
    pub folder: Option<String>,
    /// Environment the run used (name).
    pub environment: Option<String>,
    #[serde(skip)]
    pub environment_id: Option<String>,
    pub items: Vec<CollectionRunItem>,
    pub totals: CollectionRunTotals,
    /// True when `stop_on_failure` ended the run early or it was cancelled.
    pub stopped_early: bool,
    /// Accumulated variable changes to persist (in order).
    pub env_changes: Vec<VarChange>,
    pub collection_changes: Vec<VarChange>,
    /// Set when the run could not start (unknown collection/environment).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Runs the collection's requests in order. Blocking: call off the UI thread.
pub fn run_collection(
    project: &Project,
    opts: CollectionRunOptions,
    cancel: &CancelToken,
    sessions: Option<&HttpSessions>,
) -> CollectionRunReport {
    let mut report = CollectionRunReport { folder: opts.folder.clone(), ..Default::default() };
    let ci = match find_collection(project, &opts.collection) {
        Ok(ci) => ci,
        Err(e) => {
            report.error = Some(e);
            return report;
        }
    };
    let collection = &project.collections[ci];
    report.collection_id = collection.id.clone();
    report.collection = collection.name.clone();

    let mut env: Option<Environment> = match &opts.environment {
        Some(name) => match find_env(project, name) {
            Ok(ix) => Some(project.environments[ix].clone()),
            Err(e) => {
                report.error = Some(e);
                return report;
            }
        },
        None => project_ops::active_env(project).cloned(),
    };
    report.environment = env.as_ref().map(|e| e.name.clone());
    report.environment_id = env.as_ref().map(|e| e.id.clone());
    // Working copy: collection variables evolve as scripts set them.
    let mut working = project.clone();
    let started = Instant::now();

    let requests: Vec<RequestSpec> = collection
        .requests
        .iter()
        .filter(|r| opts.folder.as_deref().is_none_or(|f| in_folder(r, f)))
        .cloned()
        .collect();
    for (n, mut request) in requests.into_iter().enumerate() {
        if cancel.is_cancelled() {
            report.stopped_early = true;
            break;
        }
        request.collection_id = Some(collection.id.clone());
        let mut item = CollectionRunItem {
            request_id: request.id.clone(),
            name: request.name.clone(),
            method: request.method.full_label().to_string(),
            url: request.url.clone(),
            status: None,
            duration_ms: None,
            tests: Vec::new(),
            error: None,
            script_logs: 0,
            skipped: None,
            sent: None,
        };
        if request.is_ws() {
            item.skipped = Some("WebSocket requests are not run".into());
            report.items.push(item);
            continue;
        }
        if n > 0 && opts.delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(opts.delay_ms));
        }
        let input = project_ops::run_input(&working, &request, env.clone(), cancel, sessions);
        let output = runner::run(input);

        if let Some(env) = env.as_mut() {
            runner::apply_changes(&mut env.vars, &output.env_changes);
        }
        project_ops::apply_var_changes(&mut working, Some(&collection.id), None, &[], &output.collection_changes);
        report.env_changes.extend(output.env_changes.iter().cloned());
        report.collection_changes.extend(output.collection_changes.iter().cloned());

        if let Some(url) = &output.resolved_url {
            item.url = url.clone();
        }
        match &output.result {
            Ok(response) => {
                item.status = Some(response.status);
                item.duration_ms = Some(response.timings.total.as_millis() as u64);
            }
            Err(e) => item.error = Some(e.to_string()),
        }
        if item.error.is_none() {
            if let Some((stage, e)) = output.report.errors().next() {
                let stage = match stage {
                    super::types::ScriptStage::PreRequest => "Pre-request script",
                    super::types::ScriptStage::PostResponse => "Post-response script",
                };
                item.error = Some(match e.line {
                    Some(line) => format!("{stage}, line {line}: {}", e.message),
                    None => format!("{stage}: {}", e.message),
                });
            }
        }
        item.tests = output.report.tests().cloned().collect();
        item.script_logs = output.report.logs().count();
        item.sent = Some(output.sent);
        let failed = item.failed();
        report.items.push(item);
        if failed && opts.stop_on_failure {
            report.stopped_early = true;
            break;
        }
    }

    let totals = &mut report.totals;
    for item in &report.items {
        if item.skipped.is_some() {
            totals.skipped += 1;
            continue;
        }
        totals.requests += 1;
        totals.passed_tests += item.tests.iter().filter(|t| t.passed).count();
        totals.failed_tests += item.tests.iter().filter(|t| !t.passed).count();
        if item.failed() {
            totals.failed_requests += 1;
        }
    }
    totals.total_ms = started.elapsed().as_millis() as u64;
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{Collection, KeyValue, Method, Scripts};
    use crate::core::{EnvColor, store};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    /// Answers every request with `{"token":"t-<n>","path":"<target>","auth":"<authorization>"}`.
    fn server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for (n, stream) in listener.incoming().enumerate() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).ok();
                let target = line.split_whitespace().nth(1).unwrap_or("").to_string();
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
                let json = format!(r#"{{"token":"t-{n}","path":"{target}","auth":"{auth}"}}"#);
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                    json.len()
                );
            }
        });
        format!("http://{addr}")
    }

    fn project(base: &str) -> Project {
        let mut p = store::seed_project();
        let mut env = Environment::new("Local", EnvColor::Green);
        env.vars.push(KeyValue::new("baseUrl", base));
        p.environments.push(env);
        let mut c = Collection::new("Flow");
        let mut login = RequestSpec::new("Login", Method::Post, "{{baseUrl}}/login");
        login.scripts = Scripts {
            pre_request: String::new(),
            post_response: "pm.collectionVariables.set('auth', pm.response.json().token);\npm.test('ok', () => pm.response.to.have.status(200));".into(),
        };
        let mut me = RequestSpec::new("Me", Method::Get, "{{baseUrl}}/me?t={{auth}}");
        me.folder = Some("Users".into());
        me.scripts.post_response = "pm.test('saw token', () => pm.expect(pm.response.json().path).to.include('t-'));".into();
        let mut fail = RequestSpec::new("Fails", Method::Get, "{{baseUrl}}/x");
        fail.scripts.post_response = "pm.test('nope', () => pm.expect(1).to.equal(2));".into();
        let after = RequestSpec::new("After", Method::Get, "{{baseUrl}}/after");
        for mut r in [login, me, fail, after] {
            r.collection_id = Some(c.id.clone());
            c.requests.push(r);
        }
        p.collections.push(c);
        p
    }

    #[test]
    fn chains_variables_and_reports() {
        let base = server();
        let p = project(&base);
        let opts = CollectionRunOptions { collection: "flow".into(), ..Default::default() };
        let report = run_collection(&p, opts, &CancelToken::new(), None);
        assert!(report.error.is_none(), "{:?}", report.error);
        assert_eq!(report.items.len(), 4);
        assert_eq!(report.items[0].status, Some(200));
        let token = &report.collection_changes[0];
        assert_eq!(token.key, "auth");
        assert!(report.items[1].url.ends_with(&format!("/me?t={}", token.value.as_deref().unwrap())), "{}", report.items[1].url);
        assert!(report.items[1].tests[0].passed);
        assert_eq!(report.totals.failed_tests, 1);
        assert_eq!(report.totals.passed_tests, 2);
        assert_eq!(report.totals.failed_requests, 1);
        assert!(!report.stopped_early);
        assert_eq!(report.environment.as_deref(), Some("Local"));
    }

    #[test]
    fn stops_on_failure_and_filters_folders() {
        let base = server();
        let p = project(&base);
        let opts = CollectionRunOptions { collection: "Flow".into(), stop_on_failure: true, ..Default::default() };
        let report = run_collection(&p, opts, &CancelToken::new(), None);
        assert_eq!(report.items.len(), 3);
        assert!(report.stopped_early);

        let opts = CollectionRunOptions { collection: "Flow".into(), folder: Some("users".into()), ..Default::default() };
        let report = run_collection(&p, opts, &CancelToken::new(), None);
        assert_eq!(report.items.len(), 1);
        assert_eq!(report.items[0].name, "Me");

        let opts = CollectionRunOptions { collection: "Flow".into(), environment: Some("Nope".into()), ..Default::default() };
        assert!(run_collection(&p, opts, &CancelToken::new(), None).error.unwrap().contains("Nope"));
    }
}
