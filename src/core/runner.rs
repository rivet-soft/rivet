//! The whole "send a request" pipeline, independent of the UI:
//! pre-request script → `{{var}}` resolution → HTTP → post-response script.
//!
//! The window (`app::Workspace`) runs it on a background thread; a future MCP
//! server can call the same function to drive Relay programmatically.

use super::http::{self, CancelToken, HttpSessions};
use super::script;
use super::types::{
    Environment, KeyValue, RequestSpec, ResponseData, ScriptInput, ScriptOutcome, ScriptReport,
    LogLevel, LogLine, ScriptError, ScriptStage, SendError, Settings, VarChange,
};
use super::vars;

/// Everything needed to execute one request.
#[derive(Clone)]
pub struct RunInput {
    pub request: RequestSpec,
    /// Active environment (its own variables only).
    pub env: Option<Environment>,
    /// Variables of the request's collection; they override environment values.
    pub collection_vars: Vec<KeyValue>,
    pub settings: Settings,
    pub cancel: CancelToken,
    /// Shared cookie jars; `None` sends without cookies persistence.
    pub sessions: Option<HttpSessions>,
    pub cookie_scope: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RunOutput {
    /// The request as actually sent (after the pre-request script).
    pub sent: RequestSpec,
    /// Absolute URL after variable resolution, if it got that far.
    pub resolved_url: Option<String>,
    pub result: Result<ResponseData, SendError>,
    pub report: ScriptReport,
    /// Variable changes made by both scripts, in order, to persist.
    pub env_changes: Vec<VarChange>,
    pub collection_changes: Vec<VarChange>,
}

/// Applies `set`/`unset` changes to a variable list (enabled entries).
pub fn apply_changes(vars: &mut Vec<KeyValue>, changes: &[VarChange]) {
    for change in changes {
        match &change.value {
            Some(value) => match vars.iter_mut().find(|v| v.key == change.key) {
                Some(existing) => {
                    existing.value = value.clone();
                    existing.enabled = true;
                }
                None => vars.push(KeyValue::new(change.key.clone(), value.clone())),
            },
            None => vars.retain(|v| v.key != change.key),
        }
    }
}

/// Environment with collection variables layered on top (collection wins),
/// the scope `vars::resolve` substitutes from.
pub fn effective_env(env: Option<&Environment>, collection_vars: &[KeyValue]) -> Option<Environment> {
    if env.is_none() && collection_vars.is_empty() {
        return None;
    }
    let mut merged = env
        .cloned()
        .unwrap_or_else(|| Environment::new("Collection", Default::default()));
    for var in collection_vars {
        merged.vars.retain(|old| old.key != var.key);
        merged.vars.push(var.clone());
    }
    Some(merged)
}

fn run_script(
    stage: ScriptStage,
    source: &str,
    request: &RequestSpec,
    env: &Option<Environment>,
    collection_vars: &[KeyValue],
    response: Option<&ResponseData>,
) -> Option<ScriptOutcome> {
    if source.trim().is_empty() {
        return None;
    }
    let input = ScriptInput {
        stage,
        request: request.clone(),
        env: env.as_ref().map(|e| e.vars.clone()).unwrap_or_default(),
        collection_vars: collection_vars.to_vec(),
        response: response.cloned(),
    };
    Some(run_isolated(source.to_string(), input))
}

/// Adds a Console warning when a script set variables that have nowhere to
/// be stored (no active environment / request outside any collection).
fn warn_unstored(outcome: &mut ScriptOutcome, has_env: bool, has_collection: bool) {
    let keys = |changes: &[VarChange]| changes.iter().map(|c| c.key.as_str()).collect::<Vec<_>>().join(", ");
    if !has_env && !outcome.env_changes.is_empty() {
        outcome.logs.push(LogLine {
            level: LogLevel::Warn,
            text: format!(
                "pm.environment.set: нет активного окружения, переменные не сохранены ({}). Выберите окружение или используйте pm.collectionVariables.set.",
                keys(&outcome.env_changes)
            ),
        });
        outcome.env_changes.clear();
    }
    if !has_collection && !outcome.collection_changes.is_empty() {
        outcome.logs.push(LogLine {
            level: LogLevel::Warn,
            text: format!(
                "pm.collectionVariables.set: запрос не сохранён в коллекцию, переменные не сохранены ({}).",
                keys(&outcome.collection_changes)
            ),
        });
        outcome.collection_changes.clear();
    }
}

/// Stack for script threads. Boa's parser and interpreter recurse deeply; the
/// default (or GPUI background) thread stack can overflow on ordinary scripts,
/// and a stack overflow aborts the whole app.
const SCRIPT_STACK: usize = 64 * 1024 * 1024;

/// Runs a script on its own big-stack thread and turns engine panics into a
/// script error, so a bad script or engine bug never takes the app down.
fn run_isolated(source: String, input: ScriptInput) -> ScriptOutcome {
    let handle = std::thread::Builder::new()
        .name("rivet-script".into())
        .stack_size(SCRIPT_STACK)
        .spawn(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| script::run(&source, &input)))
        });
    let failure = |message: String| ScriptOutcome {
        error: Some(ScriptError { message, line: None }),
        ..Default::default()
    };
    match handle {
        Ok(handle) => match handle.join() {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(panic)) | Err(panic) => {
                let detail = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".into());
                failure(format!("Внутренняя ошибка движка скриптов: {detail}"))
            }
        },
        Err(e) => failure(format!("Не удалось запустить скрипт: {e}")),
    }
}

/// Runs the full pipeline. Blocking: call off the UI thread.
pub fn run(input: RunInput) -> RunOutput {
    let RunInput { request, mut env, mut collection_vars, settings, cancel, sessions, cookie_scope } = input;
    let mut report = ScriptReport::default();
    let mut env_changes = Vec::new();
    let mut collection_changes = Vec::new();
    let mut sent = request.clone();
    let has_env = env.is_some();
    let has_collection = request.collection_id.is_some();

    // 1. Pre-request script: may change variables and the request itself.
    if let Some(pre) = run_script(
        ScriptStage::PreRequest,
        &request.scripts.pre_request,
        &request,
        &env,
        &collection_vars,
        None,
    ) {
        let mut pre = pre;
        warn_unstored(&mut pre, has_env, has_collection);
        if let Some(env) = env.as_mut() {
            apply_changes(&mut env.vars, &pre.env_changes);
        }
        apply_changes(&mut collection_vars, &pre.collection_changes);
        env_changes.extend(pre.env_changes.iter().cloned());
        collection_changes.extend(pre.collection_changes.iter().cloned());
        if let Some(modified) = &pre.request {
            sent = modified.clone();
        }
        let failed = pre.error.clone();
        report.pre = Some(pre);
        if let Some(error) = failed {
            return RunOutput {
                sent,
                resolved_url: None,
                result: Err(SendError::Script { message: error.message, line: error.line }),
                report,
                env_changes,
                collection_changes,
            };
        }
    }

    // 2. Resolve variables with the (possibly updated) scopes.
    let scope = effective_env(env.as_ref(), &collection_vars);
    let resolved = match vars::resolve(&sent, scope.as_ref()) {
        Ok(r) => r,
        Err(e) => {
            return RunOutput { sent, resolved_url: None, result: Err(e), report, env_changes, collection_changes };
        }
    };
    let resolved_url = Some(resolved.url.clone());

    // 3. HTTP.
    let result = match &sessions {
        Some(sessions) => http::execute_scoped(&resolved, &settings, &cancel, sessions, cookie_scope.as_deref()),
        None => http::execute(&resolved, &settings, &cancel),
    };

    // 4. Post-response script (only when a response arrived).
    if let Ok(response) = &result {
        if let Some(post) = run_script(
            ScriptStage::PostResponse,
            &sent.scripts.post_response,
            &sent,
            &env,
            &collection_vars,
            Some(response),
        ) {
            let mut post = post;
            warn_unstored(&mut post, has_env, has_collection);
            env_changes.extend(post.env_changes.iter().cloned());
            collection_changes.extend(post.collection_changes.iter().cloned());
            report.post = Some(post);
        }
    }

    RunOutput { sent, resolved_url, result, report, env_changes, collection_changes }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_changes_sets_updates_and_unsets() {
        let mut vars = vec![KeyValue::new("a", "1").enabled(false), KeyValue::new("b", "2")];
        apply_changes(
            &mut vars,
            &[
                VarChange { key: "a".into(), value: Some("10".into()) },
                VarChange { key: "c".into(), value: Some("3".into()) },
                VarChange { key: "b".into(), value: None },
            ],
        );
        assert_eq!(vars.len(), 2);
        assert_eq!((vars[0].key.as_str(), vars[0].value.as_str(), vars[0].enabled), ("a", "10", true));
        assert_eq!((vars[1].key.as_str(), vars[1].value.as_str()), ("c", "3"));
    }

    #[test]
    fn collection_vars_override_environment() {
        let mut env = Environment::new("Dev", Default::default());
        env.vars = vec![KeyValue::new("baseUrl", "http://env"), KeyValue::new("x", "1")];
        let merged = effective_env(Some(&env), &[KeyValue::new("baseUrl", "http://col")]).unwrap();
        assert_eq!(merged.var("baseUrl"), Some("http://col"));
        assert_eq!(merged.var("x"), Some("1"));
        assert!(effective_env(None, &[]).is_none());
    }

    #[test]
    fn unstored_variables_become_console_warnings() {
        let mut outcome = ScriptOutcome {
            env_changes: vec![VarChange { key: "tokentmp".into(), value: Some("x".into()) }],
            collection_changes: vec![VarChange { key: "c".into(), value: Some("y".into()) }],
            ..Default::default()
        };
        warn_unstored(&mut outcome, false, false);
        assert!(outcome.env_changes.is_empty() && outcome.collection_changes.is_empty());
        assert_eq!(outcome.logs.len(), 2);
        assert!(outcome.logs[0].text.contains("tokentmp"));
        assert!(outcome.logs.iter().all(|l| l.level == LogLevel::Warn));

        let mut kept = ScriptOutcome {
            env_changes: vec![VarChange { key: "a".into(), value: Some("1".into()) }],
            ..Default::default()
        };
        warn_unstored(&mut kept, true, false);
        assert_eq!(kept.env_changes.len(), 1);
        assert!(kept.logs.is_empty());
    }
}
