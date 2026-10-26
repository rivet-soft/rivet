//! Headers Rivet adds to a request by itself (Postman's "auto-generated
//! headers"): from the Auth tab, the body, and the client defaults. The
//! Headers tab lists them; each one that can be switched off is recorded in
//! `RequestSpec::disabled_auto_headers` and then not sent.

use super::types::{Auth, BodyKind, RequestSpec};

/// `User-Agent` sent unless the request sets its own or switches it off.
pub const DEFAULT_USER_AGENT: &str = concat!("rivet/", env!("CARGO_PKG_VERSION"));

/// Client defaults the HTTP layer adds (lowercase name, value).
pub const CLIENT_DEFAULTS: [(&str, &str); 3] =
    [("user-agent", DEFAULT_USER_AGENT), ("accept", "*/*"), ("accept-encoding", "gzip, deflate, br")];

/// One auto-generated header as the Headers tab shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoHeader {
    pub name: String,
    /// Display value (`{{vars}}` unresolved, or a "<…>" placeholder).
    pub value: String,
    /// On unless switched off (always on when it can't be).
    pub enabled: bool,
    /// Host and Content-Length are required by HTTP.
    pub can_disable: bool,
    /// Where it comes from (hover hint).
    pub source: &'static str,
}

/// Whether the request switched off auto header `name`.
pub fn is_disabled(r: &RequestSpec, name: &str) -> bool {
    r.disabled_auto_headers.iter().any(|d| d.eq_ignore_ascii_case(name))
}

fn user_sets(r: &RequestSpec, name: &str) -> bool {
    r.headers.iter().any(|h| h.enabled && h.key.trim().eq_ignore_ascii_case(name))
}

/// Content-Type a body kind gets automatically.
pub fn body_content_type(kind: BodyKind) -> Option<&'static str> {
    match kind {
        BodyKind::None => None,
        BodyKind::Json | BodyKind::GraphQl => Some("application/json"),
        BodyKind::Text => Some("text/plain"),
        BodyKind::Form => Some("application/x-www-form-urlencoded"),
        BodyKind::Multipart => Some("multipart/form-data; boundary=<вычисляется при отправке>"),
        BodyKind::Binary => Some("<по типу файла>"),
    }
}

/// The header the Auth tab adds: (name, display value).
pub fn auth_header(r: &RequestSpec) -> Option<(String, String)> {
    match &r.auth {
        Auth::None => None,
        Auth::Inherit => Some(("Authorization".into(), "Bearer {{token}}".into())),
        Auth::Bearer { token } if !token.trim().is_empty() => {
            Some(("Authorization".into(), format!("Bearer {}", token.trim())))
        }
        Auth::Bearer { .. } => None,
        Auth::Basic { .. } => Some(("Authorization".into(), "Basic <из логина и пароля>".into())),
        Auth::ApiKey { key, value, in_query: false } if !key.trim().is_empty() => {
            Some((key.trim().to_string(), value.clone()))
        }
        Auth::ApiKey { .. } => None,
    }
}

/// Auto-generated headers of `r` in the order the Headers tab lists them.
/// A header the user sets explicitly is not auto-generated.
pub fn auto_headers(r: &RequestSpec) -> Vec<AutoHeader> {
    let mut out = Vec::new();
    let mut push = |name: String, value: String, can_disable: bool, source: &'static str| {
        if user_sets(r, &name) {
            return;
        }
        let enabled = !can_disable || !is_disabled(r, &name);
        out.push(AutoHeader { name, value, enabled, can_disable, source });
    };
    if let Some((name, value)) = auth_header(r) {
        push(name, value, true, "Из вкладки Auth");
    }
    let has_body = r.body.kind != BodyKind::None;
    if let Some(ct) = body_content_type(r.body.kind) {
        push("Content-Type".into(), ct.into(), true, "По типу тела (вкладка Body)");
    }
    if has_body {
        push("Content-Length".into(), "<вычисляется при отправке>".into(), false, "Длина тела, обязателен");
    }
    push("Host".into(), "<вычисляется при отправке>".into(), false, "Хост из URL, обязателен");
    for (name, value) in CLIENT_DEFAULTS {
        let display = match name {
            "user-agent" => "User-Agent",
            "accept" => "Accept",
            _ => "Accept-Encoding",
        };
        push(display.into(), value.into(), true, "Заголовок клиента по умолчанию");
    }
    out
}

/// Switches auto header `name` on or off for `r`.
pub fn set_enabled(r: &mut RequestSpec, name: &str, enabled: bool) {
    r.disabled_auto_headers.retain(|d| !d.eq_ignore_ascii_case(name));
    if !enabled {
        r.disabled_auto_headers.push(name.to_ascii_lowercase());
    }
}

/// Client defaults `r` switched off (for the HTTP layer).
pub fn omitted_defaults(r: &RequestSpec) -> Vec<String> {
    CLIENT_DEFAULTS
        .iter()
        .filter(|(name, _)| is_disabled(r, name))
        .map(|(name, _)| name.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{Body, KeyValue, Method};

    #[test]
    fn lists_auth_body_and_client_headers() {
        let mut r = RequestSpec::new("x", Method::Post, "https://h/x");
        r.auth = Auth::Bearer { token: "{{token}}".into() };
        r.body = Body::json("{}");
        let names: Vec<String> = auto_headers(&r).into_iter().map(|h| h.name).collect();
        assert_eq!(
            names,
            ["Authorization", "Content-Type", "Content-Length", "Host", "User-Agent", "Accept", "Accept-Encoding"]
        );
        let auth = &auto_headers(&r)[0];
        assert_eq!((auth.value.as_str(), auth.enabled, auth.can_disable), ("Bearer {{token}}", true, true));
        // A user header of the same name wins; GET without body has no Content-*.
        r.headers = vec![KeyValue::new("accept", "application/json")];
        r.body = Body::default();
        let names: Vec<String> = auto_headers(&r).into_iter().map(|h| h.name).collect();
        assert_eq!(names, ["Authorization", "Host", "User-Agent", "Accept-Encoding"]);
        // API key in a header shows under its own name.
        r.auth = Auth::ApiKey { key: "X-Key".into(), value: "{{k}}".into(), in_query: false };
        assert_eq!(auto_headers(&r)[0].name, "X-Key");
    }

    #[test]
    fn switching_off() {
        let mut r = RequestSpec::new("x", Method::Get, "https://h/x");
        set_enabled(&mut r, "User-Agent", false);
        set_enabled(&mut r, "Host", false);
        let ua = auto_headers(&r).into_iter().find(|h| h.name == "User-Agent").unwrap();
        assert!(!ua.enabled);
        let host = auto_headers(&r).into_iter().find(|h| h.name == "Host").unwrap();
        assert!(host.enabled, "Host can't be switched off");
        assert_eq!(omitted_defaults(&r), ["user-agent"]);
        set_enabled(&mut r, "user-agent", true);
        assert!(omitted_defaults(&r).is_empty());
        assert_eq!(r.disabled_auto_headers, ["host"]);
    }
}
