//! See PLAN.md (core::http).
//!
//! Blocking request execution on top of hyper 1 (HTTP/1.1 and HTTP/2) with
//! rustls (ring provider, webpki roots) on the shared Tokio runtime
//! ([`super::rt`]).
//!
//! Every hop opens its own connection through a hand-rolled connector
//! (`conn`): DNS lookup → TCP connect → optional proxy tunnel → TLS handshake
//! with ALPN, so DNS / Connect / TLS are measured on the real connection. The
//! negotiated protocol picks hyper's HTTP/1.1 or HTTP/2 client
//! ([`Settings::http_version`]: `Auto` = ALPN `h2` preferred over TLS and
//! HTTP/1.1 over plain http, `Http1` = always HTTP/1.1, `Http2` = h2 over TLS,
//! h2c with prior knowledge over plain http). TTFB is the time from the
//! connection being ready until the response head arrives (includes the HTTP/2
//! preface and writing the request); Download is the body read time.
//!
//! Proxies (`settings.proxy`): `http://[user:pass@]host:port` (scheme
//! optional) or `https://…` (TLS to the proxy). https targets are tunnelled
//! with `CONNECT`; plain http targets are sent in absolute-form over HTTP/1.1.
//!
//! Redirects are followed here, one exchange per hop (same cookie jar, so
//! Set-Cookie on a hop applies to the next), so every hop is recorded as a
//! [`RedirectHop`] with its own timings. The final timings are the last hop's
//! phases, `redirect` is the sum of earlier hops and `total` covers the chain.
//!
//! The request runs as a task on the shared runtime; the caller polls the
//! [`CancelToken`] every 20 ms and aborts the task, so cancellation returns
//! immediately even while connecting or waiting for the head.

mod conn;
mod hop;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cookie_store::CookieStore;

use super::rt;
use super::types::{Method, RedirectHop, ResolvedRequest, ResponseData, SendError, Settings};
use hop::{HopRequest, run_hop};


/// Responses larger than this are truncated.
pub const MAX_BODY_BYTES: usize = 50 * 1024 * 1024;

/// Cooperative cancellation flag shared between the UI and a running request.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Cookie jars live for the current app session and are isolated by scope.
/// Removing a session clears its cookies without waiting for an in-flight request.
pub const UNSAVED_COOKIE_SCOPE: &str = "\0unsaved-requests";

type Jar = Arc<Mutex<CookieStore>>;

#[derive(Clone, Default)]
pub struct HttpSessions {
    inner: Arc<Mutex<HashMap<String, Jar>>>,
}

impl HttpSessions {
    pub fn clear(&self, scope: &str) {
        if let Ok(mut sessions) = self.inner.lock() {
            sessions.remove(scope);
        }
    }

    pub fn clear_all(&self) {
        if let Ok(mut sessions) = self.inner.lock() {
            sessions.clear();
        }
    }

    fn get(&self, scope: &str) -> Jar {
        let mut sessions = self.inner.lock().expect("HTTP sessions lock poisoned");
        sessions.entry(scope.to_string()).or_default().clone()
    }
}

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    authority
        .rsplit_once('@')
        .map_or(authority, |(_, h)| h)
        .to_string()
}

fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// Resolves a `Location` header against the URL it was received from:
/// absolute URLs, scheme-relative (`//host/x`), absolute paths (`/x`),
/// relative paths (`x`, `../x`), query-only (`?q`) and fragment-only (`#f`).
pub fn join_url(base: &str, location: &str) -> String {
    let location = location.trim();
    let has_scheme = location.split_once(':').is_some_and(|(scheme, _)| {
        let mut chars = scheme.chars();
        chars.next().is_some_and(|c| c.is_ascii_alphabetic())
            && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    });
    if has_scheme {
        return location.to_string();
    }
    let (scheme, rest) = base.split_once("://").unwrap_or(("http", base));
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let without_fragment = tail.split('#').next().unwrap_or("");
    let path = without_fragment.split('?').next().unwrap_or("");
    let origin = format!("{scheme}://{authority}");

    if location.is_empty() {
        return format!("{origin}{without_fragment}");
    }
    if location.starts_with("//") {
        return format!("{scheme}:{location}");
    }
    if location.starts_with('#') {
        return format!("{origin}{without_fragment}{location}");
    }
    if location.starts_with('?') {
        let path = if path.is_empty() { "/" } else { path };
        return format!("{origin}{path}{location}");
    }
    let (loc_path, loc_suffix) = match location.find(['?', '#']) {
        Some(i) => location.split_at(i),
        None => (location, ""),
    };
    let merged = if loc_path.starts_with('/') {
        loc_path.to_string()
    } else {
        let dir = path.rfind('/').map_or("/", |i| &path[..=i]);
        format!("{dir}{loc_path}")
    };
    format!("{origin}{}{loc_suffix}", remove_dot_segments(&merged))
}

/// RFC 3986 §5.2.4 for an absolute path.
fn remove_dot_segments(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let segments: Vec<&str> = path.split('/').skip(1).collect();
    let last = segments.len().saturating_sub(1);
    for (i, seg) in segments.iter().enumerate() {
        match *seg {
            "." => {
                if i == last {
                    out.push("");
                }
            }
            ".." => {
                out.pop();
                if i == last {
                    out.push("");
                }
            }
            s => out.push(s),
        }
    }
    format!("/{}", out.join("/"))
}

async fn run(
    req: &ResolvedRequest,
    settings: &Settings,
    cancel: &CancelToken,
    host: &str,
    jar: &Mutex<CookieStore>,
) -> Result<ResponseData, SendError> {
    let timeout_ms = settings.timeout_ms;
    let chain_start = Instant::now();
    if cancel.is_cancelled() {
        return Err(SendError::Cancelled);
    }

    let proxy = settings
        .proxy
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(conn::parse_proxy)
        .transpose()
        .map_err(|message| SendError::Network {
            code: "EIO".into(),
            message,
            host: host.into(),
        })?;
    let overall = (timeout_ms > 0).then(|| Duration::from_millis(timeout_ms));
    let max_hops = if settings.follow_redirects {
        settings.max_redirects.clamp(1, 50) as usize
    } else {
        0
    };

    let mut method = req.method;
    let mut url = req.url.clone();
    let mut headers = req.headers.clone();
    let mut body = req.body.clone();
    let mut hops = Vec::new();
    let mut redirect_history = vec![url.clone()];

    loop {
        if cancel.is_cancelled() {
            return Err(SendError::Cancelled);
        }
        let follow = hops.len() < max_hops;
        let hop_request = HopRequest {
            method,
            url: &url,
            headers: &headers,
            body: body.as_deref(),
        };
        let exchange = run_hop(
            &hop_request,
            settings,
            proxy.as_ref(),
            jar,
            follow,
        );
        // Each hop gets whatever is left of the overall timeout.
        let response = match overall {
            Some(total) => {
                let left = total.saturating_sub(chain_start.elapsed());
                if left.is_zero() {
                    return Err(SendError::Timeout {
                        after_ms: timeout_ms,
                    });
                }
                tokio::time::timeout(left, exchange)
                    .await
                    .map_err(|_| SendError::Timeout {
                        after_ms: timeout_ms,
                    })??
            }
            None => exchange.await?,
        };
        if cancel.is_cancelled() {
            return Err(SendError::Cancelled);
        }
        let location = header_value(&response.headers, "location").map(|l| join_url(&url, l));
        let next = match location {
            Some(location) if follow && is_redirect(response.status) => location,
            _ => {
                let mut timings = response.timings;
                timings.redirect = hops.iter().map(|h: &RedirectHop| h.timings.total).sum();
                timings.total = chain_start.elapsed();
                return Ok(ResponseData {
                    status: response.status,
                    status_text: response.status_text,
                    http_version: response.http_version,
                    remote_addr: response.remote_addr,
                    headers: response.headers,
                    redirect_history,
                    hops,
                    body: response.body,
                    timings,
                });
            }
        };

        // Browser behaviour: 303 (and 301/302 after POST) become a bodyless GET.
        let to_get = method != Method::Head
            && (response.status == 303
                || (matches!(response.status, 301 | 302) && method == Method::Post));
        hops.push(RedirectHop {
            method,
            url: url.clone(),
            status: response.status,
            status_text: response.status_text,
            location: next.clone(),
            headers: response.headers,
            remote_addr: response.remote_addr,
            timings: response.timings,
        });
        if to_get {
            method = Method::Get;
            body = None;
            headers.retain(|(k, _)| {
                !k.eq_ignore_ascii_case("content-type") && !k.eq_ignore_ascii_case("content-length")
            });
        }
        if !host_of(&url).eq_ignore_ascii_case(&host_of(&next)) {
            headers.retain(|(k, _)| {
                !k.eq_ignore_ascii_case("authorization") && !k.eq_ignore_ascii_case("cookie")
            });
        }
        redirect_history.push(next.clone());
        url = next;
    }
}

/// Sends the request and blocks until the whole response is read, the timeout
/// (`settings.timeout_ms`, global) elapses or `cancel` is triggered. 4xx/5xx are
/// returned as `Ok`. Bodies are capped at [`MAX_BODY_BYTES`]. HTTP/1.1 or
/// HTTP/2 depending on [`Settings::http_version`] and ALPN.
pub fn execute(
    req: &ResolvedRequest,
    settings: &Settings,
    cancel: &CancelToken,
) -> Result<ResponseData, SendError> {
    execute_with_jar(req, settings, cancel, Jar::default())
}

/// Sends using a shared cookie jar when `cookie_scope` is supplied.
/// Without a scope, every request uses a throwaway jar.
pub fn execute_scoped(
    req: &ResolvedRequest,
    settings: &Settings,
    cancel: &CancelToken,
    sessions: &HttpSessions,
    cookie_scope: Option<&str>,
) -> Result<ResponseData, SendError> {
    let jar = cookie_scope.map(|id| sessions.get(id)).unwrap_or_default();
    execute_with_jar(req, settings, cancel, jar)
}

fn execute_with_jar(
    req: &ResolvedRequest,
    settings: &Settings,
    cancel: &CancelToken,
    jar: Jar,
) -> Result<ResponseData, SendError> {
    if cancel.is_cancelled() {
        return Err(SendError::Cancelled);
    }
    let host = host_of(&req.url);
    let (tx, rx) = mpsc::channel();
    let task = {
        let (req, settings, cancel, worker_host) =
            (req.clone(), settings.clone(), cancel.clone(), host.clone());
        rt::spawn(async move {
            let result = run(&req, &settings, &cancel, &worker_host, &jar).await;
            let _ = tx.send(result);
        })
    };
    // Safety net in case the task gets stuck past the per-hop timeouts.
    let deadline = (settings.timeout_ms > 0)
        .then(|| Instant::now() + Duration::from_millis(settings.timeout_ms + 2_000));
    loop {
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(result) => {
                return if cancel.is_cancelled() {
                    Err(SendError::Cancelled)
                } else {
                    result
                };
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(SendError::Network {
                    code: "EIO".into(),
                    message: "request task crashed".into(),
                    host,
                });
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if cancel.is_cancelled() {
                    task.abort();
                    return Err(SendError::Cancelled);
                }
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    task.abort();
                    return Err(SendError::Timeout {
                        after_ms: settings.timeout_ms,
                    });
                }
            }
        }
    }
}

/// Human-readable size: "512 B", "1.2 KB", "3.4 MB", "1.1 GB" (1024-based).
pub fn format_size(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.1} GB", b / (KB * KB * KB))
    }
}

/// "148 ms" below a second, "1.4 s" below a minute, "2m 05s" above.
pub fn format_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{:.1} s", d.as_secs_f64())
    } else {
        let s = d.as_secs();
        format!("{}m {:02}s", s / 60, s % 60)
    }
}

/// Standard reason phrase ("Created"), or "" for unknown codes.
pub fn status_text(code: u16) -> &'static str {
    match code {
        100 => "Continue",
        101 => "Switching Protocols",
        102 => "Processing",
        103 => "Early Hints",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        203 => "Non-Authoritative Information",
        204 => "No Content",
        205 => "Reset Content",
        206 => "Partial Content",
        207 => "Multi-Status",
        208 => "Already Reported",
        226 => "IM Used",
        300 => "Multiple Choices",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        305 => "Use Proxy",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Content Too Large",
        414 => "URI Too Long",
        415 => "Unsupported Media Type",
        416 => "Range Not Satisfiable",
        417 => "Expectation Failed",
        418 => "I'm a teapot",
        421 => "Misdirected Request",
        422 => "Unprocessable Content",
        423 => "Locked",
        424 => "Failed Dependency",
        425 => "Too Early",
        426 => "Upgrade Required",
        428 => "Precondition Required",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        451 => "Unavailable For Legal Reasons",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        506 => "Variant Also Negotiates",
        507 => "Insufficient Storage",
        508 => "Loop Detected",
        510 => "Not Extended",
        511 => "Network Authentication Required",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1229), "1.2 KB");
        assert_eq!(format_size(3 * 1024 * 1024 + 400 * 1024), "3.4 MB");
        assert_eq!(format_duration(Duration::from_millis(148)), "148 ms");
        assert_eq!(format_duration(Duration::from_millis(1420)), "1.4 s");
        assert_eq!(format_duration(Duration::from_secs(125)), "2m 05s");
        assert_eq!(status_text(201), "Created");
        assert_eq!(status_text(799), "");
    }

    #[test]
    fn host_parsing() {
        assert_eq!(host_of("https://u:p@api.x.com:8443/a?b"), "api.x.com:8443");
        assert_eq!(host_of("http://localhost"), "localhost");
    }

    #[test]
    fn cancel_token() {
        let t = CancelToken::default();
        let t2 = t.clone();
        assert!(!t.is_cancelled());
        t2.cancel();
        assert!(t.is_cancelled());
    }
}
