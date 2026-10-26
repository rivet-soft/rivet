//! UI-free project helpers shared by the MCP server and the window: lookups
//! by id or name, request scopes (same semantics as `Workspace::dispatch`),
//! persisting script variable changes and schema checks.

use std::time::{SystemTime, UNIX_EPOCH};

use super::http::{self, CancelToken, HttpSessions};
use super::openapi::Spec;
use super::runner::{self, RunInput};
use super::types::{
    Collection, Environment, HistoryEntry, KeyValue, Method, Mismatch, Project, RequestSpec, ResponseData,
    Settings, VarChange, new_id,
};

/// Separator of nested folder paths (`Users / Admin`).
pub const FOLDER_SEP: &str = " / ";

/// Methods that change server state and need confirmation in production.
pub fn is_destructive(method: Method) -> bool {
    matches!(method, Method::Post | Method::Put | Method::Patch | Method::Delete)
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The active environment, if any.
pub fn active_env(project: &Project) -> Option<&Environment> {
    project.environments.get(project.active_env)
}

/// Index of the environment with this id or (case-insensitive) name.
pub fn find_env(project: &Project, id_or_name: &str) -> Result<usize, String> {
    let q = id_or_name.trim();
    project
        .environments
        .iter()
        .position(|e| e.id == q)
        .or_else(|| project.environments.iter().position(|e| e.name.eq_ignore_ascii_case(q)))
        .ok_or_else(|| {
            let names: Vec<&str> = project.environments.iter().map(|e| e.name.as_str()).collect();
            format!("Environment '{q}' not found. Available: {}", list_or_none(&names))
        })
}

/// Index of the collection with this id or (case-insensitive) name.
pub fn find_collection(project: &Project, id_or_name: &str) -> Result<usize, String> {
    let q = id_or_name.trim();
    if let Some(ix) = project.collections.iter().position(|c| c.id == q) {
        return Ok(ix);
    }
    let matches: Vec<usize> = (0..project.collections.len())
        .filter(|&i| project.collections[i].name.eq_ignore_ascii_case(q))
        .collect();
    match matches.as_slice() {
        [ix] => Ok(*ix),
        [] => {
            let names: Vec<&str> = project.collections.iter().map(|c| c.name.as_str()).collect();
            Err(format!("Collection '{q}' not found. Available: {}", list_or_none(&names)))
        }
        _ => Err(format!("Several collections are named '{q}'; use the collection id")),
    }
}

fn list_or_none(names: &[&str]) -> String {
    if names.is_empty() { "(none)".into() } else { names.join(", ") }
}

/// Whether `request` lies in `folder` or one of its subfolders.
pub fn in_folder(request: &RequestSpec, folder: &str) -> bool {
    let folder = folder.trim().trim_matches('/').trim();
    if folder.is_empty() {
        return true;
    }
    request.folder.as_deref().is_some_and(|f| {
        f.eq_ignore_ascii_case(folder)
            || f.to_ascii_lowercase().starts_with(&format!("{}{FOLDER_SEP}", folder.to_ascii_lowercase()))
    })
}

/// `(collection index, request index)` of a saved request found by id, by
/// unique name, or by path `Collection / Folder / Name`. `collection` narrows
/// a name search to one collection (id or name).
pub fn find_request(project: &Project, query: &str, collection: Option<&str>) -> Result<(usize, usize), String> {
    let q = query.trim();
    for (ci, c) in project.collections.iter().enumerate() {
        if let Some(ri) = c.requests.iter().position(|r| r.id == q) {
            return Ok((ci, ri));
        }
    }
    let scope: Vec<usize> = match collection {
        Some(col) => vec![find_collection(project, col)?],
        None => (0..project.collections.len()).collect(),
    };
    let mut matches = Vec::new();
    for &ci in &scope {
        for (ri, r) in project.collections[ci].requests.iter().enumerate() {
            if r.name.eq_ignore_ascii_case(q) || path_matches(&project.collections[ci], r, q) {
                matches.push((ci, ri));
            }
        }
    }
    match matches.as_slice() {
        [one] => Ok(*one),
        [] => Err(format!("Request '{q}' not found (use list_requests to see ids)")),
        many => {
            let ids: Vec<String> = many
                .iter()
                .map(|&(ci, ri)| {
                    let r = &project.collections[ci].requests[ri];
                    format!("{} ({} {})", r.id, project.collections[ci].name, r.folder.as_deref().unwrap_or(""))
                })
                .collect();
            Err(format!("Request name '{q}' is ambiguous; use one of the ids: {}", ids.join(", ")))
        }
    }
}

/// `Collection / Folder / Name` or `Collection/Folder/Name`.
fn path_matches(collection: &Collection, request: &RequestSpec, query: &str) -> bool {
    let norm = |s: &str| {
        s.split('/').map(|p| p.trim().to_lowercase()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("/")
    };
    let mut path = collection.name.clone();
    if let Some(folder) = &request.folder {
        path.push('/');
        path.push_str(folder);
    }
    path.push('/');
    path.push_str(&request.name);
    norm(&path) == norm(query)
}

/// The variable scopes a request runs with: the active environment and its
/// collection's variables (collection values win during resolution).
pub fn request_scope(project: &Project, request: &RequestSpec) -> (Option<Environment>, Vec<KeyValue>) {
    let collection_vars = collection_of(project, request).map(|c| c.vars.clone()).unwrap_or_default();
    (active_env(project).cloned(), collection_vars)
}

pub fn collection_of<'a>(project: &'a Project, request: &RequestSpec) -> Option<&'a Collection> {
    request.collection_id.as_ref().and_then(|id| project.collections.iter().find(|c| &c.id == id))
}

/// Project settings with the request's redirect overrides applied.
pub fn request_settings(project: &Project, request: &RequestSpec) -> Settings {
    let mut settings = project.settings.clone();
    apply_request_overrides(&mut settings, request);
    settings
}

/// The request's own redirect, timeout and SSL settings over the app's.
pub fn apply_request_overrides(settings: &mut Settings, request: &RequestSpec) {
    if let Some(follow) = request.follow_redirects {
        settings.follow_redirects = follow;
    }
    if let Some(limit) = request.max_redirects {
        settings.max_redirects = limit;
    }
    if let Some(timeout) = request.timeout_ms {
        settings.timeout_ms = timeout.max(1);
    }
    if let Some(verify) = request.ssl_verify {
        settings.ssl_verify = verify;
    }
}

/// Cookie jar scope: per collection (when enabled), or the shared unsaved scope.
pub fn cookie_scope(project: &Project, request: &RequestSpec) -> Option<String> {
    if !project.settings.cookies_enabled {
        return None;
    }
    match collection_of(project, request) {
        Some(c) => c.cookies_enabled.then(|| c.id.clone()),
        None => Some(http::UNSAVED_COOKIE_SCOPE.to_string()),
    }
}

/// A ready [`RunInput`] for `request` with an explicit environment.
pub fn run_input(
    project: &Project,
    request: &RequestSpec,
    env: Option<Environment>,
    cancel: &CancelToken,
    sessions: Option<&HttpSessions>,
) -> RunInput {
    let collection_vars = collection_of(project, request).map(|c| c.vars.clone()).unwrap_or_default();
    RunInput {
        request: request.clone(),
        env,
        collection_vars,
        settings: request_settings(project, request),
        cancel: cancel.clone(),
        sessions: sessions.cloned(),
        cookie_scope: cookie_scope(project, request),
    }
}

/// Persists script variable changes: environment changes go to `env_id` (the
/// active environment when `None`), collection changes to `collection_id`.
/// Returns whether anything was applied.
pub fn apply_var_changes(
    project: &mut Project,
    collection_id: Option<&str>,
    env_id: Option<&str>,
    env_changes: &[VarChange],
    collection_changes: &[VarChange],
) -> bool {
    let mut changed = false;
    if !env_changes.is_empty() {
        let env = match env_id {
            Some(id) => project.environments.iter_mut().find(|e| e.id == id),
            None => {
                let active = project.active_env;
                project.environments.get_mut(active)
            }
        };
        if let Some(env) = env {
            runner::apply_changes(&mut env.vars, env_changes);
            changed = true;
        }
    }
    if !collection_changes.is_empty() {
        if let Some(c) = collection_id.and_then(|id| project.collections.iter_mut().find(|c| c.id == id)) {
            runner::apply_changes(&mut c.vars, collection_changes);
            changed = true;
        }
    }
    changed
}

/// A history entry for a finished send (like the window records it).
pub fn history_entry(env_name: &str, sent: RequestSpec, status: Option<u16>, duration_ms: Option<u64>) -> HistoryEntry {
    HistoryEntry { id: new_id(), at: now_secs(), env: env_name.to_string(), request: sent, status, duration_ms }
}

/// The OpenAPI spec a request is validated against: its collection's linked
/// file, otherwise the project's spec. Loaded from disk.
pub fn spec_for_request(project: &Project, request: &RequestSpec) -> Option<Spec> {
    if let Some(source) = collection_of(project, request).and_then(|c| c.source.as_ref()) {
        return Spec::load(source).ok();
    }
    project.spec_path.as_ref().and_then(|p| Spec::load(p).ok())
}

/// OpenAPI spec of a collection: its own linked file, else the project's.
pub fn spec_for_collection(project: &Project, collection: &Collection) -> Option<Spec> {
    match &collection.source {
        Some(source) => Spec::load(source).ok(),
        None => project.spec_path.as_ref().and_then(|p| Spec::load(p).ok()),
    }
}

/// Response schema label and mismatches for a request created from a spec.
pub fn check_schema(spec: &Spec, request: &RequestSpec, response: &ResponseData) -> (Option<String>, Vec<Mismatch>) {
    let Some(spec_ref) = &request.spec_ref else {
        return (None, Vec::new());
    };
    let label = spec.response_schema_label(spec_ref, response.status);
    let mismatches = serde_json::from_slice::<serde_json::Value>(&response.body)
        .ok()
        .and_then(|body| spec.validate(spec_ref, response.status, &body))
        .unwrap_or_default();
    (label, mismatches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{EnvColor, store};

    fn project() -> Project {
        let mut p = store::seed_project();
        let mut dev = Environment::new("Dev", EnvColor::Green);
        dev.vars.push(KeyValue::new("baseUrl", "http://dev"));
        let prod = Environment::new("Prod", EnvColor::Red);
        p.environments = vec![dev, prod];
        let mut c = Collection::new("Shop");
        c.vars.push(KeyValue::new("baseUrl", "http://col"));
        c.folders = vec!["Users".into(), "Users / Admin".into()];
        for (name, folder) in [("List", Some("Users")), ("Ban", Some("Users / Admin")), ("Ping", None)] {
            let mut r = RequestSpec::new(name, Method::Get, "{{baseUrl}}/x");
            r.collection_id = Some(c.id.clone());
            r.folder = folder.map(String::from);
            c.requests.push(r);
        }
        p.collections.push(c);
        p
    }

    #[test]
    fn request_overrides_win_over_app_settings() {
        let mut settings = Settings::default();
        let mut r = RequestSpec::new("x", crate::core::types::Method::Get, "http://h/");
        apply_request_overrides(&mut settings, &r);
        assert_eq!(settings, Settings::default(), "no overrides: app settings as they are");
        r.follow_redirects = Some(false);
        r.max_redirects = Some(2);
        r.timeout_ms = Some(0);
        r.ssl_verify = Some(false);
        apply_request_overrides(&mut settings, &r);
        assert!(!settings.follow_redirects);
        assert_eq!(settings.max_redirects, 2);
        assert_eq!(settings.timeout_ms, 1, "a zero timeout is clamped");
        assert!(!settings.ssl_verify);
    }

    #[test]
    fn finds_by_id_name_and_path() {
        let p = project();
        let id = p.collections[0].requests[1].id.clone();
        assert_eq!(find_request(&p, &id, None), Ok((0, 1)));
        assert_eq!(find_request(&p, "ban", None), Ok((0, 1)));
        assert_eq!(find_request(&p, "Shop / Users / Admin / Ban", None), Ok((0, 1)));
        assert_eq!(find_request(&p, "shop/ping", Some("Shop")), Ok((0, 2)));
        assert!(find_request(&p, "nope", None).unwrap_err().contains("not found"));
        assert_eq!(find_collection(&p, "shop"), Ok(0));
        assert!(find_collection(&p, "x").unwrap_err().contains("Shop"));
        assert_eq!(find_env(&p, "prod"), Ok(1));
    }

    #[test]
    fn folder_filter_includes_subfolders() {
        let p = project();
        let names = |f: &str| {
            p.collections[0].requests.iter().filter(|r| in_folder(r, f)).map(|r| r.name.clone()).collect::<Vec<_>>()
        };
        assert_eq!(names("Users"), ["List", "Ban"]);
        assert_eq!(names("users / admin"), ["Ban"]);
        assert_eq!(names(""), ["List", "Ban", "Ping"]);
    }

    #[test]
    fn scope_and_var_changes() {
        let mut p = project();
        let request = p.collections[0].requests[0].clone();
        let (env, vars) = request_scope(&p, &request);
        assert_eq!(env.unwrap().name, "Dev");
        assert_eq!(vars[0].value, "http://col");
        assert_eq!(cookie_scope(&p, &request).as_deref(), Some(p.collections[0].id.as_str()));

        let col = p.collections[0].id.clone();
        let prod = p.environments[1].id.clone();
        let set = |k: &str, v: &str| VarChange { key: k.into(), value: Some(v.into()) };
        assert!(apply_var_changes(&mut p, Some(&col), None, &[set("a", "1")], &[set("b", "2")]));
        assert_eq!(p.environments[0].var("a"), Some("1"));
        assert_eq!(p.collections[0].vars.iter().find(|v| v.key == "b").unwrap().value, "2");
        apply_var_changes(&mut p, None, Some(&prod), &[set("c", "3")], &[]);
        assert_eq!(p.environments[1].var("c"), Some("3"));
        assert!(!apply_var_changes(&mut p, None, None, &[], &[set("x", "y")]));
    }
}
