//! HTTP header name / value checks (RFC 9110): the Headers table highlights
//! what [`check_name`] / [`check_value`] report, and `vars::resolve` refuses to
//! send a request whose final headers have an error.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// The request can't be sent with it.
    Error,
    /// Sent, but probably not what was meant.
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderIssue {
    pub severity: Severity,
    pub message: String,
}

impl HeaderIssue {
    fn error(message: impl Into<String>) -> Self {
        Self { severity: Severity::Error, message: message.into() }
    }

    fn warning(message: impl Into<String>) -> Self {
        Self { severity: Severity::Warning, message: message.into() }
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// `tchar` of RFC 9110 §5.6.2.
fn is_tchar(c: char) -> bool {
    c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c)
}

/// Problem with a header name; `has_value` flags an empty name next to a value.
pub fn check_name(name: &str, has_value: bool) -> Option<HeaderIssue> {
    if name.is_empty() {
        return has_value.then(|| HeaderIssue::error("Пустое имя заголовка при заполненном значении"));
    }
    let bad = name.chars().find(|c| !is_tchar(*c))?;
    Some(HeaderIssue::error(match bad {
        ' ' | '\t' => "В имени заголовка не может быть пробелов".to_string(),
        ':' => "В имени заголовка не может быть двоеточия".to_string(),
        '\r' | '\n' => "В имени заголовка не может быть перевода строки".to_string(),
        c if c.is_control() => "В имени заголовка не может быть управляющих символов".to_string(),
        c => format!(
            "Недопустимый символ «{c}» в имени заголовка: разрешены латиница, цифры и !#$%&'*+-.^_`|~"
        ),
    }))
}

/// Problem with a header value: line breaks, NUL and other control characters
/// (tab is fine) are errors; blanks at the edges and non-ASCII are warnings.
pub fn check_value(value: &str) -> Option<HeaderIssue> {
    if value.contains(['\r', '\n']) {
        return Some(HeaderIssue::error("В заголовке не может быть перевода строки"));
    }
    if value.contains('\0') {
        return Some(HeaderIssue::error("В заголовке не может быть символа NUL"));
    }
    if value.chars().any(|c| c.is_control() && c != '\t') {
        return Some(HeaderIssue::error("В заголовке не может быть управляющих символов"));
    }
    if value.starts_with([' ', '\t']) || value.ends_with([' ', '\t']) {
        return Some(HeaderIssue::warning(
            "Пробелы в начале или в конце значения: сервер их, скорее всего, отбросит",
        ));
    }
    if !value.is_ascii() {
        return Some(HeaderIssue::warning(
            "Не-ASCII символы (например, кириллица) уйдут как UTF-8: не все серверы их примут",
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(check_name("X-Request-Id", true), None);
        assert_eq!(check_name("a!#$%&'*+-.^_`|~9", true), None);
        assert_eq!(check_name("", false), None);
        let msg = |n: &str| check_name(n, true).map(|i| i.message);
        assert_eq!(msg("").unwrap(), "Пустое имя заголовка при заполненном значении");
        assert_eq!(msg("X Token").unwrap(), "В имени заголовка не может быть пробелов");
        assert_eq!(msg("Host:").unwrap(), "В имени заголовка не может быть двоеточия");
        assert_eq!(msg("A\nB").unwrap(), "В имени заголовка не может быть перевода строки");
        assert_eq!(msg("A\u{1}").unwrap(), "В имени заголовка не может быть управляющих символов");
        assert!(msg("Ключ").unwrap().contains("«К»"));
        assert!(msg("{{h}}").unwrap().contains("«{»"));
        assert!(check_name("X Token", true).unwrap().is_error());
    }

    #[test]
    fn values() {
        assert_eq!(check_value("Bearer a.b.c"), None);
        assert_eq!(check_value("a\tb"), None);
        assert_eq!(check_value(""), None);
        let err = |v: &str| check_value(v).filter(HeaderIssue::is_error).map(|i| i.message);
        assert_eq!(err("a\r\nX-Evil: 1").unwrap(), "В заголовке не может быть перевода строки");
        assert_eq!(err("a\nb").unwrap(), "В заголовке не может быть перевода строки");
        assert_eq!(err("a\0").unwrap(), "В заголовке не может быть символа NUL");
        assert_eq!(err("a\u{7f}").unwrap(), "В заголовке не может быть управляющих символов");
        let warn = check_value(" token ").unwrap();
        assert_eq!(warn.severity, Severity::Warning);
        assert!(warn.message.contains("Пробелы"));
        assert_eq!(check_value("привет").unwrap().severity, Severity::Warning);
    }
}
