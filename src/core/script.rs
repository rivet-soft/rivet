//! Request scripts (Postman-compatible `pm.*`) on the Boa JavaScript engine.
//!
//! The Boa surface is deliberately tiny so the engine can be swapped later:
//! two string globals in (`__input_json`, `__response_body`), three `eval`s
//! (prelude, user script, `__finish()`), one JSON string out. Everything else
//! (`pm`, `console`, chai-lite `expect`, legacy globals) lives in
//! `script_prelude.js`.

use std::path::Path;
use std::time::Instant;

use boa_engine::property::Attribute;
use boa_engine::{Context, JsError, JsString, JsValue, Script, Source, js_string};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::core::types::{
    BodyKind, KeyValue, LogLine, Method, RequestSpec, ScriptError, ScriptInput, ScriptOutcome,
    ScriptStage, TestResult, VarChange,
};

const PRELUDE: &str = include_str!("script_prelude.js");
const PRELUDE_PATH: &str = "rivet:prelude.js";
const SCRIPT_PATH: &str = "rivet:script.js";

/// Loop iterations allowed per function call before the script is stopped.
pub const LOOP_ITERATION_LIMIT: u64 = 5_000_000;
/// Maximum JavaScript call depth.
pub const RECURSION_LIMIT: usize = 512;
/// Maximum bracket nesting of a script. Boa's parser is recursive and needs
/// roughly 100 KB (debug) / 20 KB (release) of native stack per level, so
/// absurdly nested sources are rejected before parsing instead of
/// overflowing the stack (which aborts the process).
pub const MAX_NESTING: usize = 256;

/// Runs `script` for `input` (blocking; call off the UI thread).
/// Runs `script` for `input` on a dedicated big-stack thread (Boa recurses
/// deeply; callers' threads — tests, GPUI executors — may have small stacks).
pub fn run(script: &str, input: &ScriptInput) -> ScriptOutcome {
    let (script, input) = (script.to_string(), input.clone());
    std::thread::Builder::new()
        .name("rivet-script".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || run_on_current_thread(&script, &input))
        .ok()
        .and_then(|h| h.join().ok())
        .unwrap_or_else(|| ScriptOutcome {
            error: Some(crate::core::types::ScriptError { message: "script engine crashed".into(), line: None }),
            ..Default::default()
        })
}

fn run_on_current_thread(script: &str, input: &ScriptInput) -> ScriptOutcome {
    let started = Instant::now();
    // Never panic into the caller: an engine panic becomes a ScriptError.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_inner(script, input)));
    let mut outcome = result.unwrap_or_else(|panic| {
        let reason = panic
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".into());
        ScriptOutcome {
            request: (input.stage == ScriptStage::PreRequest).then(|| input.request.clone()),
            error: Some(error(format!("script engine crashed: {reason}"), None)),
            ..ScriptOutcome::default()
        }
    });
    outcome.duration = started.elapsed();
    outcome
}

fn run_inner(script: &str, input: &ScriptInput) -> ScriptOutcome {
    let pre = input.stage == ScriptStage::PreRequest;
    let mut outcome = ScriptOutcome {
        request: pre.then(|| input.request.clone()),
        ..ScriptOutcome::default()
    };

    let mut context = Context::default();
    context
        .runtime_limits_mut()
        .set_loop_iteration_limit(LOOP_ITERATION_LIMIT);
    context
        .runtime_limits_mut()
        .set_recursion_limit(RECURSION_LIMIT);

    let body: JsValue = match &input.response {
        Some(response) => JsString::from(response.body_text().as_str()).into(),
        None => JsValue::null(),
    };
    let globals = [
        (
            js_string!("__input_json"),
            JsValue::from(JsString::from(input_json(input).as_str())),
        ),
        (js_string!("__response_body"), body),
    ];
    for (key, value) in globals {
        if let Err(e) = context.register_global_property(key, value, Attribute::all()) {
            outcome.error = Some(internal_error(&e));
            return outcome;
        }
    }

    if let Err(e) = context.eval(Source::from_bytes(PRELUDE).with_path(Path::new(PRELUDE_PATH))) {
        outcome.error = Some(internal_error(&e));
        return outcome;
    }

    // The user script is its own `Source`, evaluated at top level and unwrapped,
    // so Boa's positions for `rivet:script.js` are the user's own lines.
    let max_line = script.lines().count().max(1) as u32;
    if let Some(line) = too_deeply_nested(script) {
        outcome.error = Some(error(
            format!("SyntaxError: script is nested more than {MAX_NESTING} levels deep"),
            Some(line),
        ));
        return outcome;
    }
    let source = Source::from_bytes(script).with_path(Path::new(SCRIPT_PATH));
    match Script::parse(source, None, &mut context) {
        Err(e) => outcome.error = Some(syntax_error(&e, max_line)),
        Ok(parsed) => {
            if let Err(e) = parsed.evaluate(&mut context) {
                outcome.error = Some(script_error(e, &mut context, script, max_line));
            }
        }
    }

    // Collect what the script did, even after an error (Postman keeps
    // variables set and tests run before the failure).
    let finished = context
        .eval(Source::from_bytes("__finish()"))
        .map_err(|e| internal_error(&e))
        .and_then(|v| {
            v.as_string()
                .map(|s| s.to_std_string_lossy())
                .ok_or_else(|| error("script runtime returned no result", None))
        })
        .and_then(|text| {
            serde_json::from_str::<Output>(&text)
                .map_err(|e| error(format!("invalid script result: {e}"), None))
        });
    match finished {
        Ok(out) => {
            outcome.env_changes = out.env;
            outcome.collection_changes = out.collection;
            outcome.tests = out.tests;
            outcome.logs = out.logs;
            if let (Some(changes), Some(request)) = (out.request, outcome.request.as_mut()) {
                apply_request(request, changes);
            }
        }
        Err(e) => {
            if outcome.error.is_none() {
                outcome.error = Some(e);
            }
        }
    }
    outcome
}

/// Line where bracket nesting first exceeds [`MAX_NESTING`]. A rough scan:
/// skips strings and comments, may over-count brackets inside regex literals.
fn too_deeply_nested(script: &str) -> Option<u32> {
    let mut depth = 0usize;
    let mut line = 1u32;
    let mut chars = script.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' => line += 1,
            '(' | '[' | '{' => {
                depth += 1;
                if depth > MAX_NESTING {
                    return Some(line);
                }
            }
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            '\'' | '"' | '`' => {
                while let Some(s) = chars.next() {
                    match s {
                        '\\' => {
                            chars.next();
                        }
                        '\n' => {
                            line += 1;
                            if c != '`' {
                                break;
                            }
                        }
                        s if s == c => break,
                        _ => {}
                    }
                }
            }
            '/' if chars.peek() == Some(&'/') => {
                for s in chars.by_ref() {
                    if s == '\n' {
                        line += 1;
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = ' ';
                for s in chars.by_ref() {
                    if s == '\n' {
                        line += 1;
                    }
                    if prev == '*' && s == '/' {
                        break;
                    }
                    prev = s;
                }
            }
            _ => {}
        }
    }
    None
}

// ---- input -------------------------------------------------------------------

fn vars_object(vars: &[KeyValue]) -> Value {
    let mut map = Map::new();
    for kv in vars.iter().filter(|kv| kv.enabled && !kv.key.is_empty()) {
        map.insert(kv.key.clone(), Value::String(kv.value.clone()));
    }
    Value::Object(map)
}

fn kv_array(rows: &[KeyValue]) -> Value {
    rows.iter()
        .map(|kv| json!({ "key": kv.key, "value": kv.value, "enabled": kv.enabled }))
        .collect()
}

fn body_mode(kind: BodyKind) -> &'static str {
    match kind {
        BodyKind::None => "none",
        BodyKind::Json | BodyKind::Text => "raw",
        BodyKind::Form => "urlencoded",
        BodyKind::Multipart => "formdata",
        BodyKind::Binary => "file",
    }
}

fn input_json(input: &ScriptInput) -> String {
    let r = &input.request;
    let response = input.response.as_ref().map(|res| {
        json!({
            "code": res.status,
            "status": res.status_text,
            "headers": res.headers.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
            "responseTime": res.timings.total.as_millis() as u64,
            "size": res.body.len(),
        })
    });
    json!({
        "stage": match input.stage {
            ScriptStage::PreRequest => "prerequest",
            ScriptStage::PostResponse => "test",
        },
        "request": {
            "id": r.id,
            "name": r.name,
            "method": r.method.full_label(),
            "url": r.url,
            "headers": kv_array(&r.headers),
            "params": kv_array(&r.params),
            "body": { "mode": body_mode(r.body.kind), "raw": r.body.text },
        },
        "env": vars_object(&input.env),
        "collection": vars_object(&input.collection_vars),
        "response": response,
    })
    .to_string()
}

// ---- output ------------------------------------------------------------------

#[derive(Deserialize)]
struct Output {
    env: Vec<VarChange>,
    collection: Vec<VarChange>,
    tests: Vec<TestResult>,
    logs: Vec<LogLine>,
    request: Option<RequestChanges>,
}

#[derive(Deserialize)]
struct RequestChanges {
    url: String,
    method: String,
    headers: Vec<HeaderOut>,
    body_raw: Option<String>,
}

#[derive(Deserialize)]
struct HeaderOut {
    key: String,
    value: String,
    enabled: bool,
    /// Index into the original `RequestSpec::headers` (keeps descriptions).
    index: Option<usize>,
}

fn apply_request(request: &mut RequestSpec, changes: RequestChanges) {
    request.url = changes.url;
    if let Some(method) = Method::parse(&changes.method) {
        request.method = method;
    }
    let original = std::mem::take(&mut request.headers);
    request.headers = changes
        .headers
        .into_iter()
        .map(|h| {
            let mut row = h
                .index
                .and_then(|i| original.get(i).cloned())
                .unwrap_or_else(|| KeyValue::new("", ""));
            row.key = h.key;
            row.value = h.value;
            row.enabled = h.enabled;
            row
        })
        .collect();
    if let Some(raw) = changes.body_raw {
        if !matches!(request.body.kind, BodyKind::Json | BodyKind::Text) {
            let json_header = request.headers.iter().any(|h| {
                h.enabled
                    && h.key.eq_ignore_ascii_case("content-type")
                    && h.value.to_ascii_lowercase().contains("json")
            });
            request.body.kind = if json_header {
                BodyKind::Json
            } else {
                BodyKind::Text
            };
        }
        request.body.text = raw;
    }
}

// ---- errors ------------------------------------------------------------------

fn error(message: impl Into<String>, line: Option<u32>) -> ScriptError {
    ScriptError {
        message: message.into(),
        line,
    }
}

/// A failure of the runtime itself (prelude, result collection).
fn internal_error(e: &JsError) -> ScriptError {
    error(format!("script runtime error: {e}"), None)
}

/// A parse error of the user script: "... at line N, col M".
fn syntax_error(e: &JsError, max_line: u32) -> ScriptError {
    let message = e
        .as_native()
        .map(|n| n.message().to_string())
        .unwrap_or_else(|| e.to_string());
    // No position means the script ended early (e.g. an unclosed brace).
    let line = parse_after(&message, " at line ").map_or(max_line, |l| l.clamp(1, max_line));
    let message = message
        .rfind(" at line ")
        .map_or(message.as_str(), |i| &message[..i]);
    error(format!("SyntaxError: {message}"), Some(line))
}

/// Maps a runtime error of the user script to a message and a 1-based line.
/// The innermost `rivet:script.js:LINE:COL` frame of the error's backtrace is
/// used (errors thrown inside the prelude, e.g. by `pm.response.json()`, point
/// at the user's calling line). Boa leaves top-level unresolved references
/// without a reliable position, so "X is not defined" uses the first line
/// that mentions identifier X.
fn script_error(e: JsError, context: &mut Context, script: &str, max_line: u32) -> ScriptError {
    let clamp_line = |line: u32| line.clamp(1, max_line);
    let display = e.to_string();
    let script_line = |text: &str| parse_after(text, &format!("{SCRIPT_PATH}:")).map(clamp_line);
    if e.as_engine().is_some() {
        // Runtime limits (loop iterations, recursion) and engine panics.
        let line = script_line(&display);
        let message = if display.contains("loop") {
            format!("Script stopped: exceeded {LOOP_ITERATION_LIMIT} loop iterations (infinite loop?)")
        } else if display.contains("recursi") {
            format!("Script stopped: exceeded call depth of {RECURSION_LIMIT} (infinite recursion?)")
        } else {
            display.lines().next().unwrap_or("engine error").to_string()
        };
        return error(message, line);
    }

    let line_from_display = script_line(&display);
    match e.into_opaque(context) {
        Ok(value) => {
            let (message, stack) = describe_thrown(&value, context);
            // Boa gives top-level unresolved references no (or a stale)
            // position, so the identifier search wins for those.
            let line = undefined_reference_line(&message, script)
                .or_else(|| stack.as_deref().and_then(|s| script_line(s)))
                .or(line_from_display);
            error(message, line)
        }
        Err(_) => error(display, line_from_display),
    }
}

/// `name: message` and the `stack` string of a thrown value.
fn describe_thrown(value: &JsValue, context: &mut Context) -> (String, Option<String>) {
    let Some(object) = value.as_object() else {
        let text = value
            .to_string(context)
            .map(|s| s.to_std_string_lossy())
            .unwrap_or_else(|_| "error".into());
        return (format!("Uncaught {text}"), None);
    };
    let mut get = |key: JsString| {
        object
            .get(key, context)
            .ok()
            .filter(|v| !v.is_undefined() && !v.is_null())
            .and_then(|v| v.to_string(context).ok())
            .map(|s| s.to_std_string_lossy())
    };
    let name = get(js_string!("name"));
    let message = get(js_string!("message"));
    let stack = get(js_string!("stack"));
    let text = match (name, message) {
        (Some(n), Some(m)) if !m.is_empty() => format!("{n}: {m}"),
        (Some(n), _) => n,
        (None, Some(m)) => m,
        (None, None) => value
            .to_string(context)
            .map(|s| s.to_std_string_lossy())
            .unwrap_or_else(|_| "error".into()),
    };
    (text, stack)
}

/// Line of the first use of `X` for a "ReferenceError: X is not defined".
fn undefined_reference_line(message: &str, script: &str) -> Option<u32> {
    let name = message
        .strip_prefix("ReferenceError: ")?
        .strip_suffix(" is not defined")?;
    let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    script.lines().enumerate().find_map(|(i, line)| {
        let found = line.match_indices(name).any(|(at, _)| {
            let before = line[..at].chars().next_back();
            let after = line[at + name.len()..].chars().next();
            !before.is_some_and(|c| is_ident(c) || c == '.') && !after.is_some_and(is_ident)
        });
        found.then_some(i as u32 + 1)
    })
}

fn parse_after(text: &str, marker: &str) -> Option<u32> {
    let start = text.find(marker)? + marker.len();
    let digits: String = text[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}
