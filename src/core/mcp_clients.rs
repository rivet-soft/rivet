//! Connecting AI assistants to `rivet mcp`: the command to run, ready-to-paste
//! snippets and one-click installers for Claude Code, Claude Desktop and Cursor.
//! UI-free; the installers do file and process I/O, so the app runs them off
//! the UI thread.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Map, Value, json};

/// Name of the server entry in the clients' configs.
pub const SERVER_NAME: &str = "rivet";

/// A supported MCP client.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum McpClient {
    ClaudeCode,
    ClaudeDesktop,
    Cursor,
}

impl McpClient {
    pub const ALL: [McpClient; 3] = [McpClient::ClaudeCode, McpClient::ClaudeDesktop, McpClient::Cursor];

    pub fn label(self) -> &'static str {
        match self {
            McpClient::ClaudeCode => "Claude Code",
            McpClient::ClaudeDesktop => "Claude Desktop",
            McpClient::Cursor => "Cursor",
        }
    }
}

/// How a client starts the server: executable and arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RivetCommand {
    pub exe: PathBuf,
    pub args: Vec<String>,
}

impl RivetCommand {
    /// The running executable (inside `Rivet.app/Contents/MacOS` when launched
    /// from the bundle) with `mcp`.
    pub fn current() -> Self {
        let (exe, args) = rivet_command();
        Self { exe, args }
    }

    pub fn exe_display(&self) -> String {
        self.exe.display().to_string()
    }
}

/// `(current executable, ["mcp"])`. On Unix symlinks are resolved so the path
/// stays valid when launched through one (e.g. `/usr/local/bin/rivet`); on
/// Windows the path is kept as is (canonical paths there get a `\\?\` prefix).
pub fn rivet_command() -> (PathBuf, Vec<String>) {
    let exe = std::env::current_exe()
        .map(|p| if cfg!(windows) { p } else { fs::canonicalize(&p).unwrap_or(p) })
        .unwrap_or_else(|_| PathBuf::from("rivet"));
    (exe, vec!["mcp".to_string()])
}

fn quote(arg: &str) -> String {
    if !arg.is_empty() && arg.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=@".contains(c)) {
        arg.to_string()
    } else {
        format!("\"{}\"", arg.replace('"', "\\\""))
    }
}

/// `claude mcp add rivet -- "<exe>" mcp`.
pub fn claude_code_snippet(command: &RivetCommand) -> String {
    let mut s = format!("claude mcp add {SERVER_NAME} -- \"{}\"", command.exe.display());
    for a in &command.args {
        s.push(' ');
        s.push_str(&quote(a));
    }
    s
}

/// The `mcpServers.rivet` entry.
fn server_entry(command: &RivetCommand) -> Value {
    json!({"command": command.exe.display().to_string(), "args": command.args})
}

/// `{"mcpServers": {"rivet": {"command": …, "args": ["mcp"]}}}` for Claude Desktop / Cursor.
pub fn json_snippet(command: &RivetCommand) -> String {
    let value = json!({"mcpServers": {SERVER_NAME: server_entry(command)}});
    serde_json::to_string_pretty(&value).unwrap_or_default()
}

/// What "Копировать" puts on the clipboard for `client`.
pub fn snippet(client: McpClient, command: &RivetCommand) -> String {
    match client {
        McpClient::ClaudeCode => claude_code_snippet(command),
        McpClient::ClaudeDesktop | McpClient::Cursor => json_snippet(command),
    }
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(std::env::temp_dir)
}

/// Default `claude_desktop_config.json`: `~/Library/Application Support/Claude`
/// (macOS), `%APPDATA%\Claude` (Windows), `~/.config/Claude` (Linux).
pub fn claude_desktop_config_path() -> PathBuf {
    let dir = if cfg!(target_os = "macos") {
        home().join("Library").join("Application Support")
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| home().join("AppData").join("Roaming"))
    } else {
        home().join(".config")
    };
    dir.join("Claude").join("claude_desktop_config.json")
}

/// `~/.cursor/mcp.json`.
pub fn cursor_config_path() -> PathBuf {
    home().join(".cursor").join("mcp.json")
}

/// `~/.claude.json`: Claude Code's user config (user-scope servers live in `mcpServers`).
pub fn claude_code_config_path() -> PathBuf {
    home().join(".claude.json")
}

/// Where the installers and the status check look. `Default` is the user's real
/// locations; tests point it at temp files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpClientPaths {
    pub claude_desktop: PathBuf,
    pub cursor: PathBuf,
    pub claude_code_config: PathBuf,
    /// The `claude` CLI; `None` = search PATH and the usual install locations.
    pub claude_cli: Option<PathBuf>,
}

impl Default for McpClientPaths {
    fn default() -> Self {
        Self {
            claude_desktop: claude_desktop_config_path(),
            cursor: cursor_config_path(),
            claude_code_config: claude_code_config_path(),
            claude_cli: None,
        }
    }
}

impl McpClientPaths {
    /// The config file shown in the install confirmation.
    pub fn config_path(&self, client: McpClient) -> PathBuf {
        match client {
            McpClient::ClaudeCode => self.claude_code_config.clone(),
            McpClient::ClaudeDesktop => self.claude_desktop.clone(),
            McpClient::Cursor => self.cursor.clone(),
        }
    }
}

fn read_config(path: &Path) -> Result<Option<Map<String, Value>>, String> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(Some(Map::new()));
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Ok(Some(map)),
        Ok(_) => Err(format!("{}: ожидался JSON-объект; файл не изменён", path.display())),
        Err(e) => Err(format!("{}: некорректный JSON ({e}); файл не изменён", path.display())),
    }
}

/// Sets `mcpServers.rivet` in the JSON config at `path`, keeping every other
/// key and server. Creates the file (and directories) when missing; the
/// previous file is kept as `<name>.bak`; the write is atomic. An invalid
/// existing file is an error and is left untouched.
pub fn install_json_config(path: &Path, command: &RivetCommand) -> Result<PathBuf, String> {
    let existing = read_config(path)?;
    let mut root = existing.clone().unwrap_or_default();
    let servers = root.entry("mcpServers").or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(servers) = servers else {
        return Err(format!("{}: mcpServers — не объект; файл не изменён", path.display()));
    };
    servers.insert(SERVER_NAME.to_string(), server_entry(command));
    let mut text = serde_json::to_string_pretty(&Value::Object(root)).map_err(|e| e.to_string())?;
    text.push('\n');

    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    if existing.is_some() {
        fs::copy(path, backup_path(path)).map_err(|e| format!("{}: {e}", backup_path(path).display()))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp-{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })?;
    Ok(path.to_path_buf())
}

/// `<path>.bak`, e.g. `claude_desktop_config.json.bak`.
pub fn backup_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".bak");
    PathBuf::from(s)
}

/// Adds Rivet to Claude Desktop's config (`None` = the default location).
pub fn install_claude_desktop(config_path: Option<PathBuf>) -> Result<PathBuf, String> {
    install_json_config(&config_path.unwrap_or_else(claude_desktop_config_path), &RivetCommand::current())
}

/// Adds Rivet to Cursor's global `mcp.json` (`None` = `~/.cursor/mcp.json`).
pub fn install_cursor(config_path: Option<PathBuf>) -> Result<PathBuf, String> {
    install_json_config(&config_path.unwrap_or_else(cursor_config_path), &RivetCommand::current())
}

/// Whether the JSON config at `path` has `mcpServers.rivet` (top level, or in
/// any `projects.*` entry as Claude Code stores local-scope servers).
pub fn json_config_has_rivet(path: &Path) -> bool {
    let Ok(Some(root)) = read_config(path) else {
        return false;
    };
    let has = |obj: &Map<String, Value>| obj.get("mcpServers").and_then(|s| s.get(SERVER_NAME)).is_some();
    has(&root)
        || root
            .get("projects")
            .and_then(Value::as_object)
            .is_some_and(|projects| projects.values().filter_map(Value::as_object).any(has))
}

/// The `claude` CLI: PATH first, then the usual install locations (apps
/// started from Finder get a minimal PATH).
pub fn find_claude_cli() -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) { &["claude.exe", "claude.cmd", "claude"] } else { &["claude"] };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    let h = home();
    dirs.extend([
        h.join(".claude").join("local"),
        h.join(".local").join("bin"),
        h.join(".npm-global").join("bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("npm"));
    }
    dirs.iter().flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file())
}

/// A `claude` invocation whose PATH also has the CLI's own directory and the
/// Homebrew ones, so a `#!/usr/bin/env node` launcher finds node.
fn claude(cli: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(cli);
    cmd.args(args);
    let mut path: Vec<PathBuf> = cli.parent().map(Path::to_path_buf).into_iter().collect();
    path.extend(std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default());
    if !cfg!(windows) {
        path.extend([PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin")]);
    }
    if let Ok(joined) = std::env::join_paths(path) {
        cmd.env("PATH", joined);
    }
    cmd
}

fn output_text(out: &std::process::Output) -> String {
    let mut text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let err = String::from_utf8_lossy(&out.stderr);
    if !err.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(err.trim());
    }
    text
}

/// `claude mcp add --scope user rivet -- <exe> mcp` with the given CLI. An
/// existing user-scope `rivet` entry is removed first so the path is current.
pub fn install_claude_code_with(cli: &Path, command: &RivetCommand) -> Result<String, String> {
    // Ignore the result: there may be nothing to remove.
    let _ = claude(cli, &["mcp", "remove", SERVER_NAME, "--scope", "user"]).output();
    let exe = command.exe.display().to_string();
    let mut args = vec!["mcp", "add", "--scope", "user", SERVER_NAME, "--", exe.as_str()];
    args.extend(command.args.iter().map(String::as_str));
    let out = claude(cli, &args).output().map_err(|e| format!("{}: {e}", cli.display()))?;
    let text = output_text(&out);
    if out.status.success() {
        Ok(if text.is_empty() { format!("Rivet добавлен в Claude Code ({})", cli.display()) } else { text })
    } else {
        Err(if text.is_empty() { format!("claude mcp add завершился с кодом {:?}", out.status.code()) } else { text })
    }
}

/// Adds Rivet to Claude Code (user scope) through the `claude` CLI. Blocking:
/// call it off the UI thread.
pub fn install_claude_code() -> Result<String, String> {
    let cli = find_claude_cli().ok_or_else(|| {
        "Claude Code CLI (claude) не найден. Установите его или выполните скопированную команду в терминале.".to_string()
    })?;
    install_claude_code_with(&cli, &RivetCommand::current())
}

/// Runs the installer for `client` with `paths` (blocking).
pub fn install(client: McpClient, paths: &McpClientPaths, command: &RivetCommand) -> Result<String, String> {
    match client {
        McpClient::ClaudeCode => {
            let cli = paths.claude_cli.clone().or_else(find_claude_cli).ok_or_else(|| {
                "Claude Code CLI (claude) не найден. Установите его или выполните скопированную команду в терминале.".to_string()
            })?;
            install_claude_code_with(&cli, command)
        }
        McpClient::ClaudeDesktop | McpClient::Cursor => {
            let path = install_json_config(&paths.config_path(client), command)?;
            let restart = if client == McpClient::ClaudeDesktop { " Перезапустите Claude Desktop." } else { "" };
            Ok(format!("Rivet добавлен в {} ({}).{restart}", client.label(), path.display()))
        }
    }
}

/// Which clients have a `rivet` server configured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct McpStatus {
    pub claude_code: bool,
    pub claude_desktop: bool,
    pub cursor: bool,
}

impl McpStatus {
    pub fn get(&self, client: McpClient) -> bool {
        match client {
            McpClient::ClaudeCode => self.claude_code,
            McpClient::ClaudeDesktop => self.claude_desktop,
            McpClient::Cursor => self.cursor,
        }
    }
}

/// Reads the clients' configs (no processes are started). Claude Code is
/// detected from `~/.claude.json`.
pub fn detect_status(paths: &McpClientPaths) -> McpStatus {
    McpStatus {
        claude_code: json_config_has_rivet(&paths.claude_code_config),
        claude_desktop: json_config_has_rivet(&paths.claude_desktop),
        cursor: json_config_has_rivet(&paths.cursor),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd() -> RivetCommand {
        RivetCommand { exe: PathBuf::from("/Applications/Rivet.app/Contents/MacOS/Rivet"), args: vec!["mcp".into()] }
    }

    #[test]
    fn snippets() {
        assert_eq!(
            claude_code_snippet(&cmd()),
            "claude mcp add rivet -- \"/Applications/Rivet.app/Contents/MacOS/Rivet\" mcp"
        );
        let v: Value = serde_json::from_str(&json_snippet(&cmd())).unwrap();
        assert_eq!(v, json!({"mcpServers": {"rivet": {"command": "/Applications/Rivet.app/Contents/MacOS/Rivet", "args": ["mcp"]}}}));
        assert_eq!(snippet(McpClient::Cursor, &cmd()), json_snippet(&cmd()));
        let (exe, args) = rivet_command();
        assert!(exe.is_absolute());
        assert_eq!(args, ["mcp"]);
    }

    #[test]
    fn install_creates_missing_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Claude").join("claude_desktop_config.json");
        assert!(!json_config_has_rivet(&path));
        assert_eq!(install_json_config(&path, &cmd()).unwrap(), path);
        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["rivet"]["args"], json!(["mcp"]));
        assert!(!backup_path(&path).exists());
        assert!(json_config_has_rivet(&path));
    }

    #[test]
    fn install_merges_and_keeps_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        let before = r#"{"theme":"dark","mcpServers":{"other":{"command":"x","args":[]},"rivet":{"command":"old"}}}"#;
        fs::write(&path, before).unwrap();
        install_json_config(&path, &cmd()).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["mcpServers"]["other"], json!({"command": "x", "args": []}));
        assert_eq!(v["mcpServers"]["rivet"]["command"], "/Applications/Rivet.app/Contents/MacOS/Rivet");
        assert_eq!(fs::read_to_string(backup_path(&path)).unwrap(), before);
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["theme", "mcpServers"]);
    }

    #[test]
    fn invalid_config_is_refused_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claude_desktop_config.json");
        fs::write(&path, "{ not json").unwrap();
        let err = install_json_config(&path, &cmd()).unwrap_err();
        assert!(err.contains("некорректный JSON"), "{err}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");
        assert!(!backup_path(&path).exists());

        fs::write(&path, r#"{"mcpServers": []}"#).unwrap();
        assert!(install_json_config(&path, &cmd()).is_err());
        fs::write(&path, "[1]").unwrap();
        assert!(install_json_config(&path, &cmd()).is_err());
    }

    #[test]
    fn status_reads_configs() {
        let dir = tempfile::tempdir().unwrap();
        let paths = McpClientPaths {
            claude_desktop: dir.path().join("desktop.json"),
            cursor: dir.path().join("cursor.json"),
            claude_code_config: dir.path().join(".claude.json"),
            claude_cli: None,
        };
        assert_eq!(detect_status(&paths), McpStatus::default());
        fs::write(&paths.claude_code_config, r#"{"projects":{"/x":{"mcpServers":{"rivet":{}}}}}"#).unwrap();
        install(McpClient::Cursor, &paths, &cmd()).unwrap();
        let status = detect_status(&paths);
        assert!(status.claude_code && status.cursor && !status.claude_desktop);
        assert!(status.get(McpClient::Cursor));
    }

    #[cfg(unix)]
    #[test]
    fn claude_code_installer_runs_cli() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let cli = dir.path().join("claude");
        fs::write(&cli, format!("#!/bin/sh\necho \"$@\" >> \"{}\"\necho added\n", log.display())).unwrap();
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
        let out = install_claude_code_with(&cli, &cmd()).unwrap();
        assert_eq!(out, "added");
        let calls = fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = calls.lines().collect();
        assert_eq!(lines[0], "mcp remove rivet --scope user");
        assert_eq!(lines[1], "mcp add --scope user rivet -- /Applications/Rivet.app/Contents/MacOS/Rivet mcp");

        fs::write(&cli, "#!/bin/sh\necho nope >&2\nexit 3\n").unwrap();
        assert_eq!(install_claude_code_with(&cli, &cmd()).unwrap_err(), "nope");
    }
}
