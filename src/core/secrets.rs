//! Secret detection and masking shared by the environment panel and the MCP
//! server: values of secret-looking keys are hidden unless explicitly revealed.

use super::types::{Auth, KeyValue, RequestSpec};

/// Placeholder shown instead of a secret value. Sending it back in an update
/// (MCP `save_request`) keeps the stored value.
pub const MASK: &str = "••••••••";

/// Keys whose values are masked by default (`token`, `password`, `apiSecret`,
/// `Authorization`…).
pub fn is_secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    ["token", "password", "secret", "apikey", "api_key", "api-key", "passwd", "authorization"]
        .iter()
        .any(|s| k.contains(s))
}

/// True when `value` holds nothing but `{{variable}}` references (optionally
/// after an auth scheme word), so showing it reveals no secret.
fn only_references(value: &str) -> bool {
    let mut rest = String::new();
    let mut tail = value;
    while let Some(start) = tail.find("{{") {
        rest.push_str(&tail[..start]);
        match tail[start..].find("}}") {
            Some(end) => tail = &tail[start + end + 2..],
            None => {
                rest.push_str(&tail[start..]);
                tail = "";
            }
        }
    }
    rest.push_str(tail);
    let rest = rest.trim();
    value.contains("{{") && (rest.is_empty() || ["bearer", "basic", "token"].contains(&rest.to_ascii_lowercase().as_str()))
}

/// `value` masked unless it is empty or only references variables.
pub fn mask(value: &str) -> String {
    if value.is_empty() || only_references(value) {
        value.to_string()
    } else {
        MASK.to_string()
    }
}

/// Masks values of secret-looking keys in place.
pub fn mask_key_values(list: &mut [KeyValue]) {
    for kv in list.iter_mut().filter(|kv| is_secret_key(&kv.key)) {
        kv.value = mask(&kv.value);
    }
}

/// Masks `(name, value)` pairs (e.g. response headers) with secret-looking names.
pub fn mask_pairs(list: &mut [(String, String)]) {
    for (_, v) in list.iter_mut().filter(|(k, _)| is_secret_key(k)) {
        *v = mask(v);
    }
}

/// Masks credentials in a request: secret headers/params/form fields and auth.
pub fn mask_request(request: &mut RequestSpec) {
    mask_key_values(&mut request.params);
    mask_key_values(&mut request.headers);
    mask_key_values(&mut request.body.form);
    match &mut request.auth {
        Auth::Bearer { token } => *token = mask(token),
        Auth::Basic { password, .. } => *password = mask(password),
        Auth::ApiKey { value, .. } => *value = mask(value),
        Auth::None | Auth::Inherit => {}
    }
}

/// Undoes masking on an update: rows whose value is [`MASK`] take the value of
/// the same key in `previous`.
pub fn unmask_key_values(list: &mut [KeyValue], previous: &[KeyValue]) {
    for kv in list.iter_mut().filter(|kv| kv.value == MASK) {
        if let Some(old) = previous.iter().find(|old| old.key == kv.key) {
            kv.value = old.value.clone();
        }
    }
}

/// Restores masked fields of `request` from `previous` (the stored version).
pub fn unmask_request(request: &mut RequestSpec, previous: &RequestSpec) {
    unmask_key_values(&mut request.params, &previous.params);
    unmask_key_values(&mut request.headers, &previous.headers);
    unmask_key_values(&mut request.body.form, &previous.body.form);
    match (&mut request.auth, &previous.auth) {
        (Auth::Bearer { token }, Auth::Bearer { token: old }) if token == MASK => *token = old.clone(),
        (Auth::Basic { password, .. }, Auth::Basic { password: old, .. }) if password == MASK => {
            *password = old.clone()
        }
        (Auth::ApiKey { value, .. }, Auth::ApiKey { value: old, .. }) if value == MASK => *value = old.clone(),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Method;

    #[test]
    fn detects_secret_keys() {
        for key in ["token", "accessToken", "PASSWORD", "apiSecret", "x-api-key", "ApiKey", "api_key", "Authorization"] {
            assert!(is_secret_key(key), "{key}");
        }
        assert!(!is_secret_key("baseUrl"));
        assert!(!is_secret_key("Content-Type"));
    }

    #[test]
    fn masks_literals_but_not_references() {
        assert_eq!(mask("abc"), MASK);
        assert_eq!(mask(""), "");
        assert_eq!(mask("{{token}}"), "{{token}}");
        assert_eq!(mask("Bearer {{token}}"), "Bearer {{token}}");
        assert_eq!(mask("Bearer abc{{x}}"), MASK);
    }

    #[test]
    fn mask_and_unmask_round_trip() {
        let mut stored = RequestSpec::new("r", Method::Get, "http://x");
        stored.headers = vec![KeyValue::new("Authorization", "Bearer s3cr3t"), KeyValue::new("Accept", "*/*")];
        stored.auth = Auth::Basic { username: "bob".into(), password: "pw".into() };
        let mut shown = stored.clone();
        mask_request(&mut shown);
        assert_eq!(shown.headers[0].value, MASK);
        assert_eq!(shown.headers[1].value, "*/*");
        assert_eq!(shown.auth, Auth::Basic { username: "bob".into(), password: MASK.into() });
        unmask_request(&mut shown, &stored);
        assert_eq!(shown, stored);
    }
}
