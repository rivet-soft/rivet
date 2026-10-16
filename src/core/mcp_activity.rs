//! MCP activity log: one JSON line per tool call, written by `rivet mcp` next to
//! `project.json` (`mcp-activity.jsonl`) and shown in Settings → MCP.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Lines kept in the log.
pub const ACTIVITY_CAP: usize = 500;
/// The file is trimmed back to [`ACTIVITY_CAP`] once it grows this much past it,
/// so most appends don't rewrite it.
const TRIM_SLACK: usize = 50;
/// Longest stored summary, in characters.
const SUMMARY_MAX: usize = 200;

pub const FILE_NAME: &str = "mcp-activity.jsonl";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityEntry {
    /// Unix time in milliseconds.
    pub at: u64,
    pub tool: String,
    pub ok: bool,
    /// Short human-readable description (secrets masked), e.g. `GET https://… → 200`.
    pub summary: String,
    /// `clientInfo.name` from `initialize`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
}

impl ActivityEntry {
    pub fn new(tool: impl Into<String>, ok: bool, summary: impl Into<String>, client: Option<String>) -> Self {
        Self { at: now_ms(), tool: tool.into(), ok, summary: shorten(&summary.into()), client }
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// One line, at most [`SUMMARY_MAX`] characters.
fn shorten(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if line.chars().count() > SUMMARY_MAX {
        let mut s: String = line.chars().take(SUMMARY_MAX - 1).collect();
        s.push('…');
        s
    } else {
        line.to_string()
    }
}

/// `mcp-activity.jsonl` in the directory of `project_path`.
pub fn activity_path(project_path: &Path) -> PathBuf {
    match project_path.parent().filter(|d| !d.as_os_str().is_empty()) {
        Some(dir) => dir.join(FILE_NAME),
        None => PathBuf::from(FILE_NAME),
    }
}

/// Appends `entry`; trims the file to the last [`ACTIVITY_CAP`] lines when it
/// has grown past the cap (plus some slack).
pub fn append(path: &Path, entry: &ActivityEntry) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut line = serde_json::to_string(entry).map_err(|e| e.to_string())?;
    line.push('\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(line.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
    drop(file);
    trim(path, ACTIVITY_CAP, TRIM_SLACK)
}

fn trim(path: &Path, cap: usize, slack: usize) -> Result<(), String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() <= cap + slack {
        return Ok(());
    }
    let mut kept = lines[lines.len() - cap..].join("\n");
    kept.push('\n');
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp-{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, kept).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })
}

/// The last `limit` entries, newest first. A missing file is an empty log;
/// unreadable lines are skipped.
pub fn read(path: &Path, limit: usize) -> Vec<ActivityEntry> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<ActivityEntry>(l).ok())
        .take(limit)
        .collect()
}

/// Empties the log.
pub fn clear(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// `HH:MM:SS` (UTC, like the history panel) of a millisecond timestamp.
pub fn format_time(at_ms: u64) -> String {
    let s = (at_ms / 1000) % 86_400;
    format!("{:02}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_read_newest_first_and_clear() {
        let dir = tempfile::tempdir().unwrap();
        let path = activity_path(&dir.path().join("project.json"));
        assert_eq!(path, dir.path().join(FILE_NAME));
        assert!(read(&path, 10).is_empty());
        append(&path, &ActivityEntry::new("list_collections", true, "2 коллекции", Some("claude-code".into()))).unwrap();
        append(&path, &ActivityEntry::new("send_request", false, "boom\nsecond line", None)).unwrap();
        let entries = read(&path, 10);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].tool, "send_request");
        assert!(!entries[0].ok);
        assert_eq!(entries[0].summary, "boom");
        assert_eq!(entries[1].client.as_deref(), Some("claude-code"));
        assert_eq!(read(&path, 1).len(), 1);
        clear(&path).unwrap();
        assert!(read(&path, 10).is_empty());
        clear(&path).unwrap();
    }

    #[test]
    fn log_is_capped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        for i in 0..(ACTIVITY_CAP + TRIM_SLACK + 5) {
            append(&path, &ActivityEntry::new("t", true, format!("#{i}"), None)).unwrap();
        }
        let lines = fs::read_to_string(&path).unwrap().lines().count();
        assert!(lines <= ACTIVITY_CAP + TRIM_SLACK, "{lines}");
        let all = read(&path, usize::MAX);
        assert_eq!(all[0].summary, format!("#{}", ACTIVITY_CAP + TRIM_SLACK + 4));
        assert_eq!(all.len(), lines);
    }

    #[test]
    fn summary_is_shortened_and_time_formatted() {
        let e = ActivityEntry::new("t", true, "x".repeat(500), None);
        assert_eq!(e.summary.chars().count(), SUMMARY_MAX);
        assert_eq!(format_time(((13 * 3600 + 5 * 60 + 9) * 1000) as u64), "13:05:09");
    }
}
