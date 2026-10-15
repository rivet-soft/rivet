//! Project persistence (PLAN.md, core::store): one pretty-printed JSON file,
//! written atomically, with a seed project on first launch or corruption.

use std::fs;
use std::path::{Path, PathBuf};

use super::types::{HistoryEntry, Project, Settings};

/// Maximum number of history entries kept.
pub const HISTORY_CAP: usize = 500;

/// Product name, also the data directory name.
pub const APP_NAME: &str = "Rivet";
/// Previous product name; its data directory is migrated on first launch.
const LEGACY_APP_NAME: &str = "Relay";

fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir)
}

/// `<data dir>/Rivet/project.json`, e.g. `~/Library/Application Support/Rivet/project.json`.
pub fn default_path() -> PathBuf {
    data_dir().join(APP_NAME).join("project.json")
}

/// The project file used before the rename to Rivet.
pub fn legacy_path() -> PathBuf {
    data_dir().join(LEGACY_APP_NAME).join("project.json")
}

/// Copies the legacy project to `path` when `path` doesn't exist yet. The old
/// file is left in place as a backup. Returns whether a copy happened.
pub fn migrate_legacy(path: &Path, legacy: &Path) -> bool {
    if path.exists() || !legacy.exists() {
        return false;
    }
    if let Some(dir) = path.parent() {
        if fs::create_dir_all(dir).is_err() {
            return false;
        }
    }
    fs::copy(legacy, path).is_ok()
}

pub fn load(path: &Path) -> Result<Project, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Writes `project` to a sibling temp file and renames it over `path`, so a crash
/// never leaves a half-written project. Parent directories are created.
pub fn save(path: &Path, project: &Project) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let json = serde_json::to_string_pretty(project).map_err(|e| e.to_string())?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp-{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, json).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })
}

/// Loads the project, or returns [`seed_project`] when the file is missing or
/// unreadable. A corrupt file is first copied to `project.json.bak`.
pub fn load_or_default(path: &Path) -> Project {
    if !path.exists() {
        return seed_project();
    }
    match load(path) {
        Ok(p) => p,
        Err(_) => {
            let mut bak = path.as_os_str().to_owned();
            bak.push(".bak");
            let _ = fs::copy(path, PathBuf::from(bak));
            seed_project()
        }
    }
}

/// Inserts `entry` as the newest item and drops anything beyond [`HISTORY_CAP`].
pub fn push_history(project: &mut Project, entry: HistoryEntry) {
    project.history.insert(0, entry);
    project.history.truncate(HISTORY_CAP);
}

/// Empty first-launch project. The example API can be imported explicitly.
pub fn seed_project() -> Project {
    Project {
        name: APP_NAME.into(),
        spec_path: None,
        environments: Vec::new(),
        active_env: 0,
        collections: Vec::new(),
        history: Vec::new(),
        settings: Settings::default(),
        open_tabs: Vec::new(),
        active_tab: None,
    }
}
