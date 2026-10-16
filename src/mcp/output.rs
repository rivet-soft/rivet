//! JSON shapes returned by the tools (responses, requests, variables), with
//! secret masking applied unless the caller asked to reveal secrets.

use std::time::Duration;

use serde_json::{Value, json};

use crate::core::secrets::{self, is_secret_key};
use crate::core::{KeyValue, Mismatch, RedirectHop, RequestSpec, ResponseData, ScriptReport, ScriptStage, VarChange};

/// Milliseconds with two decimals.
pub fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 100_000.0).round() / 100.0
}

/// URL with secret-looking query parameter values masked.
pub fn mask_url(url: &str, reveal: bool) -> String {
    if reveal {
        return url.to_string();
    }
    let Some((base, rest)) = url.split_once('?') else {
        return url.to_string();
    };
    let (query, fragment) = rest.split_once('#').map_or((rest, None), |(q, f)| (q, Some(f)));
    let query: Vec<String> = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) if is_secret_key(k) => format!("{k}={}", secrets::mask(v)),
            _ => pair.to_string(),
        })
        .collect();
    let mut out = format!("{base}?{}", query.join("&"));
    if let Some(f) = fragment {
        out.push('#');
        out.push_str(f);
    }
    out
}

pub fn request(request: &RequestSpec, reveal: bool) -> Value {
    let mut r = request.clone();
    if !reveal {
        secrets::mask_request(&mut r);
        r.url = mask_url(&r.url, false);
    }
    serde_json::to_value(&r).unwrap_or(Value::Null)
}

pub fn variables(vars: &[KeyValue], reveal: bool) -> Value {
    Value::Array(
        vars.iter()
            .map(|v| {
                let secret = is_secret_key(&v.key);
                let value = if secret && !reveal { secrets::mask(&v.value) } else { v.value.clone() };
                let mut o = json!({"key": v.key, "value": value, "enabled": v.enabled});
                if secret {
                    o["secret"] = json!(true);
                }
                if !v.description.is_empty() {
                    o["description"] = json!(v.description);
                }
                o
            })
            .collect(),
    )
}

pub fn var_changes(changes: &[VarChange], reveal: bool) -> Value {
    Value::Array(
        changes
            .iter()
            .map(|c| match &c.value {
                Some(v) => {
                    let v = if !reveal && is_secret_key(&c.key) { secrets::mask(v) } else { v.clone() };
                    json!({"key": c.key, "set": v})
                }
                None => json!({"key": c.key, "unset": true}),
            })
            .collect(),
    )
}

fn headers(list: &[(String, String)], reveal: bool) -> Value {
    let mut list = list.to_vec();
    if !reveal {
        secrets::mask_pairs(&mut list);
    }
    Value::Array(list.into_iter().map(|(name, value)| json!({"name": name, "value": value})).collect())
}

fn hop(h: &RedirectHop, reveal: bool) -> Value {
    json!({
        "method": h.method.full_label(),
        "url": mask_url(&h.url, reveal),
        "status": h.status,
        "location": mask_url(&h.location, reveal),
        "ms": ms(h.timings.total),
    })
}

fn stage(s: ScriptStage) -> &'static str {
    match s {
        ScriptStage::PreRequest => "pre_request",
        ScriptStage::PostResponse => "post_response",
    }
}

/// Tests, console lines and errors of both script stages.
pub fn script_report(report: &ScriptReport) -> Value {
    let logs: Vec<Value> = report
        .logs()
        .map(|(s, l)| json!({"stage": stage(s), "level": l.level, "text": l.text}))
        .collect();
    let errors: Vec<Value> = report
        .errors()
        .map(|(s, e)| json!({"stage": stage(s), "message": e.message, "line": e.line}))
        .collect();
    let tests: Vec<Value> = report.tests().map(|t| json!(t)).collect();
    json!({"tests": tests, "logs": logs, "errors": errors})
}

pub fn mismatches(list: &[Mismatch]) -> Value {
    Value::Array(
        list.iter()
            .map(|m| json!({"path": m.path, "title": m.title, "expected": m.expected, "actual": m.actual}))
            .collect(),
    )
}

/// Response status, headers, body (text truncated to `max_chars`, binary as
/// base64), timings and redirect hops.
pub fn response(r: &ResponseData, max_chars: usize, reveal: bool) -> Value {
    let mut out = json!({
        "status": r.status,
        "status_text": r.status_text,
        "http_version": r.http_version,
        "remote_addr": r.remote_addr,
        "headers": headers(&r.headers, reveal),
        "content_type": r.content_type(),
        "body_size_bytes": r.body.len(),
        "timings_ms": {
            "redirect": ms(r.timings.redirect),
            "dns": ms(r.timings.dns),
            "connect": ms(r.timings.connect),
            "tls": ms(r.timings.tls),
            "ttfb": ms(r.timings.ttfb),
            "download": ms(r.timings.download),
            "total": ms(r.timings.total),
        },
        "redirects": r.hops.iter().map(|h| hop(h, reveal)).collect::<Vec<_>>(),
    });
    match std::str::from_utf8(&r.body) {
        Ok(text) => {
            let total = text.chars().count();
            let truncated = total > max_chars;
            let body: String = if truncated { text.chars().take(max_chars).collect() } else { text.to_string() };
            out["body"] = json!(body);
            out["body_truncated"] = json!(truncated);
            if truncated {
                out["body_chars"] = json!(total);
            }
        }
        Err(_) => {
            // 3 bytes → 4 base64 chars; keep the encoded text within max_chars.
            let take = (max_chars / 4 * 3).min(r.body.len());
            out["body"] = Value::Null;
            out["body_base64"] = json!(base64(&r.body[..take]));
            out["body_truncated"] = json!(take < r.body.len());
            out["body_note"] = json!(format!(
                "Binary body ({} bytes), shown as base64{}",
                r.body.len(),
                if take < r.body.len() { format!(" (first {take} bytes)") } else { String::new() }
            ));
        }
    }
    out
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn masks_secret_query_params() {
        assert_eq!(
            mask_url("http://x/a?api_key=abc&q=1#f", false),
            format!("http://x/a?api_key={}&q=1#f", secrets::MASK)
        );
        assert_eq!(mask_url("http://x/a?token={{t}}", false), "http://x/a?token={{t}}");
        assert_eq!(mask_url("http://x/a?api_key=abc", true), "http://x/a?api_key=abc");
    }
}
