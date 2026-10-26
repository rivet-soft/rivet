//! Project persistence (PLAN.md, core::store): one pretty-printed JSON file,
//! written atomically, with a seed project on first launch or corruption.

use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::types::{Collection, Environment, HistoryEntry, KeyValue, Project, Settings};

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
    save_text(path, project).map(|_| ())
}

/// [`save`] that returns the JSON text it wrote (for change detection).
pub fn save_text(path: &Path, project: &Project) -> Result<String, String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let json = serde_json::to_string_pretty(project).map_err(|e| e.to_string())?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp-{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, &json).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })?;
    Ok(json)
}

/// Size and modification time of a file: cheap to poll for external changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStamp {
    pub len: u64,
    pub modified: Option<SystemTime>,
}

/// `None` when the file doesn't exist (or can't be stat'ed).
pub fn file_stamp(path: &Path) -> Option<FileStamp> {
    let meta = fs::metadata(path).ok()?;
    Some(FileStamp { len: meta.len(), modified: meta.modified().ok() })
}

/// Hash of a file's bytes, to tell a real change from a touched file.
pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// Three-way merge of a project changed both in memory (`local`) and on disk
/// (`disk`) since both were equal to `base`.
///
/// Each side's changes are kept; when both changed the same thing, the disk
/// version wins. "The same thing" is as fine-grained as practical: an
/// environment or collection (by id) is merged field by field, its variables by
/// key, a collection's requests by id and its folders as a set; settings,
/// `name`, `spec_path` and `active_env` are single values. History entries from
/// both sides are kept (by id, newest first, capped at [`HISTORY_CAP`]).
/// `open_tabs` / `active_tab` belong to the app and are taken from `local`.
pub fn merge_projects(base: &Project, local: &Project, disk: &Project) -> Project {
    let mut history = merge_keyed(
        &base.history,
        &local.history,
        &disk.history,
        |h| &h.id,
        |_, _, d| d.clone(),
    );
    history.sort_by_key(|h| std::cmp::Reverse(h.at));
    history.truncate(HISTORY_CAP);
    Project {
        name: pick(&base.name, &local.name, &disk.name),
        spec_path: pick(&base.spec_path, &local.spec_path, &disk.spec_path),
        environments: merge_keyed(
            &base.environments,
            &local.environments,
            &disk.environments,
            |e| &e.id,
            merge_environment,
        ),
        active_env: pick(&base.active_env, &local.active_env, &disk.active_env),
        collections: merge_keyed(
            &base.collections,
            &local.collections,
            &disk.collections,
            |c| &c.id,
            merge_collection,
        ),
        history,
        settings: pick(&base.settings, &local.settings, &disk.settings),
        open_tabs: local.open_tabs.clone(),
        active_tab: local.active_tab,
        sidebar_width: local.sidebar_width,
    }
}

/// The side that changed; disk when both did.
fn pick<T: Clone + PartialEq>(base: &T, local: &T, disk: &T) -> T {
    if disk != base { disk.clone() } else { local.clone() }
}

fn merge_vars(base: &[KeyValue], local: &[KeyValue], disk: &[KeyValue]) -> Vec<KeyValue> {
    merge_keyed(base, local, disk, |v| &v.key, |_, _, d| d.clone())
}

fn merge_environment(base: &Environment, local: &Environment, disk: &Environment) -> Environment {
    Environment {
        id: disk.id.clone(),
        name: pick(&base.name, &local.name, &disk.name),
        color: pick(&base.color, &local.color, &disk.color),
        production: pick(&base.production, &local.production, &disk.production),
        vars: merge_vars(&base.vars, &local.vars, &disk.vars),
    }
}

fn merge_collection(base: &Collection, local: &Collection, disk: &Collection) -> Collection {
    let mut folders: Vec<String> = disk
        .folders
        .iter()
        // Removed locally (and untouched on disk).
        .filter(|f| local.folders.contains(f) || !base.folders.contains(f))
        .cloned()
        .collect();
    for f in &local.folders {
        if !folders.contains(f) && !base.folders.contains(f) {
            folders.push(f.clone());
        }
    }
    Collection {
        id: disk.id.clone(),
        name: pick(&base.name, &local.name, &disk.name),
        requests: merge_keyed(
            &base.requests,
            &local.requests,
            &disk.requests,
            |r| &r.id,
            |_, _, d| d.clone(),
        ),
        folders,
        vars: merge_vars(&base.vars, &local.vars, &disk.vars),
        source: pick(&base.source, &local.source, &disk.source),
        cookies_enabled: pick(&base.cookies_enabled, &local.cookies_enabled, &disk.cookies_enabled),
        description: pick(&base.description, &local.description, &disk.description),
    }
}

/// Three-way merge of a list of items identified by `key`, in disk order with
/// items added locally appended. `both` resolves an item changed on both sides.
/// An item deleted on one side and changed on the other is kept.
fn merge_keyed<T: Clone + PartialEq>(
    base: &[T],
    local: &[T],
    disk: &[T],
    key: impl Fn(&T) -> &String,
    both: impl Fn(&T, &T, &T) -> T,
) -> Vec<T> {
    if local == base {
        return disk.to_vec();
    }
    if disk == base || disk == local {
        return local.to_vec();
    }
    let find = |list: &'_ [T], k: &String| list.iter().position(|x| key(x) == k);
    let mut out = Vec::with_capacity(disk.len().max(local.len()));
    for d in disk {
        let k = key(d);
        match (find(base, k).map(|i| &base[i]), find(local, k).map(|i| &local[i])) {
            (Some(b), Some(l)) if l == b => out.push(d.clone()),
            (Some(b), Some(l)) if d == b => out.push(l.clone()),
            (Some(b), Some(l)) => out.push(both(b, l, d)),
            // Deleted locally; keep it only if it changed on disk since.
            (Some(b), None) => {
                if d != b {
                    out.push(d.clone());
                }
            }
            // Added on disk (or on both sides: disk wins).
            (None, _) => out.push(d.clone()),
        }
    }
    for l in local {
        let k = key(l);
        if find(disk, k).is_some() {
            continue;
        }
        match find(base, k).map(|i| &base[i]) {
            // Added locally.
            None => out.push(l.clone()),
            // Deleted on disk; keep it only if it changed locally since.
            Some(b) => {
                if l != b {
                    out.push(l.clone());
                }
            }
        }
    }
    out
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
        sidebar_width: None,
    }
}
