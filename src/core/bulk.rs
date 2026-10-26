//! Bulk edit of params / headers as text (like Postman): one `key:value` per
//! line, a `//` prefix marks a disabled row.

use super::types::KeyValue;

/// The rows as bulk text. Rows with neither key nor value are skipped.
pub fn to_text(rows: &[KeyValue]) -> String {
    rows.iter()
        .filter(|kv| !kv.key.is_empty() || !kv.value.is_empty())
        .map(|kv| {
            let prefix = if kv.enabled { "" } else { "//" };
            format!("{prefix}{}:{}", kv.key, kv.value)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parses bulk text back into rows. The key is everything before the first
/// `:` (a line without one is a key with an empty value); blank lines are
/// skipped. Descriptions and the multipart `file` flag are carried over from
/// `previous` rows with the same key, in order, since the text can't hold them.
pub fn from_text(text: &str, previous: &[KeyValue]) -> Vec<KeyValue> {
    let mut used = vec![false; previous.len()];
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (enabled, line) = match line.strip_prefix("//") {
                Some(rest) => (false, rest.trim_start()),
                None => (true, line),
            };
            if line.is_empty() {
                return None;
            }
            let (key, value) = line.split_once(':').unwrap_or((line, ""));
            let mut kv = KeyValue::new(key.trim(), value.trim());
            kv.enabled = enabled;
            if let Some(ix) = (0..previous.len()).find(|&i| !used[i] && previous[i].key == kv.key) {
                used[ix] = true;
                kv.description = previous[ix].description.clone();
                kv.file = previous[ix].file;
            }
            Some(kv)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_postman_style_text() {
        let rows = from_text(
            "access_token:{{access_token}}\nv:{{v}}\n//restore_sid:\nnew_password:\n\n  // off : x \nurl:http://h:8080/a\nflag",
            &[],
        );
        let got: Vec<(bool, &str, &str)> =
            rows.iter().map(|kv| (kv.enabled, kv.key.as_str(), kv.value.as_str())).collect();
        assert_eq!(
            got,
            vec![
                (true, "access_token", "{{access_token}}"),
                (true, "v", "{{v}}"),
                (false, "restore_sid", ""),
                (true, "new_password", ""),
                (false, "off", "x"),
                (true, "url", "http://h:8080/a"),
                (true, "flag", ""),
            ]
        );
    }

    #[test]
    fn round_trip_keeps_order_enabled_and_descriptions() {
        let mut rows = vec![
            KeyValue::new("b", "2").enabled(false),
            KeyValue::new("a", "x:y"),
            KeyValue::new("a", "dup"),
            KeyValue::new("", ""),
        ];
        rows[1].description = "first a".into();
        rows[2].description = "second a".into();
        let text = to_text(&rows);
        assert_eq!(text, "//b:2\na:x:y\na:dup");
        let back = from_text(&text, &rows);
        assert_eq!(back, rows[..3].to_vec());
    }
}
