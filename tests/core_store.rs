use rivet::core::store::{self, HISTORY_CAP};
use rivet::core::types::{Collection, EnvColor, Environment, HistoryEntry, KeyValue, RequestSpec};
use rivet::model::Method;

fn entry(at: u64) -> HistoryEntry {
    HistoryEntry {
        id: format!("h{at}"),
        at,
        env: "Dev".into(),
        request: RequestSpec::new("List", Method::Get, "{{baseUrl}}/orders"),
        status: Some(200),
        duration_ms: Some(12),
    }
}

#[test]
fn save_load_roundtrip_creates_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/deeper/project.json");
    let mut p = store::seed_project();
    p.environments
        .push(Environment::new("Dev", EnvColor::Green));
    p.environments[0]
        .vars
        .push(KeyValue::new("extra", "1").description("d"));
    store::push_history(&mut p, entry(5));
    p.open_tabs.push(RequestSpec::untitled());
    p.active_tab = Some(0);

    store::save(&path, &p).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\n  \"name\""), "pretty printed");
    assert_eq!(store::load(&path).unwrap(), p);
    assert_eq!(store::load_or_default(&path), p);

    // No temp files left behind.
    let mut names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    names.sort();
    assert_eq!(names, ["history.json", "project.json"]);

    // Overwrite works.
    p.name = "Renamed".into();
    store::save(&path, &p).unwrap();
    assert_eq!(store::load(&path).unwrap().name, "Renamed");
}

#[test]
fn old_project_defaults_redirects_and_collection_cookies() {
    let mut project = store::seed_project();
    project.collections.push(Collection::new("Legacy"));
    project.open_tabs.push(RequestSpec::untitled());
    let mut json = serde_json::to_value(project).unwrap();
    json["settings"]
        .as_object_mut()
        .unwrap()
        .remove("follow_redirects");
    json["settings"]
        .as_object_mut()
        .unwrap()
        .remove("max_redirects");
    json["settings"]
        .as_object_mut()
        .unwrap()
        .remove("cookies_enabled");
    json["collections"][0]
        .as_object_mut()
        .unwrap()
        .remove("cookies_enabled");
    json["open_tabs"][0]
        .as_object_mut()
        .unwrap()
        .remove("follow_redirects");
    json["open_tabs"][0]
        .as_object_mut()
        .unwrap()
        .remove("max_redirects");
    let loaded: rivet::core::Project = serde_json::from_value(json).unwrap();
    assert!(loaded.settings.follow_redirects);
    assert_eq!(loaded.settings.max_redirects, 10);
    assert!(loaded.settings.cookies_enabled);
    assert!(loaded.collections[0].cookies_enabled);
    assert_eq!(loaded.open_tabs[0].follow_redirects, None);
}

#[test]
fn missing_file_gives_seed_and_load_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.json");
    assert!(store::load(&path).is_err());
    let p = store::load_or_default(&path);
    assert_eq!(p.name, "Rivet");
    assert!(!dir.path().join("project.json.bak").exists());
}

#[test]
fn corrupt_file_gives_default_and_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.json");
    std::fs::write(&path, "{ not json").unwrap();
    assert!(store::load(&path).is_err());
    let p = store::load_or_default(&path);
    assert_eq!(p.name, "Rivet");
    let bak = dir.path().join("project.json.bak");
    assert_eq!(std::fs::read_to_string(bak).unwrap(), "{ not json");
}

#[test]
fn history_newest_first_and_capped() {
    let mut p = store::seed_project();
    for at in 0..(HISTORY_CAP as u64 + 20) {
        store::push_history(&mut p, entry(at));
    }
    assert_eq!(p.history.len(), HISTORY_CAP);
    assert_eq!(p.history[0].at, HISTORY_CAP as u64 + 19);
    assert_eq!(p.history.last().unwrap().at, 20);
    assert!(p.history.windows(2).all(|w| w[0].at > w[1].at));
}

#[test]
fn seed_contents() {
    let p = store::seed_project();
    assert_eq!(p.name, "Rivet");
    assert!(p.spec_path.is_none());
    assert!(p.environments.is_empty());
    assert_eq!(p.active_env, 0);
    assert!(p.collections.is_empty());
    assert!(p.history.is_empty());
    assert!(p.open_tabs.is_empty());
    assert_eq!(p.settings, Default::default());
}

#[test]
fn default_path_is_under_rivet() {
    let p = store::default_path();
    assert!(p.ends_with("Rivet/project.json"));
}

#[test]
fn legacy_relay_project_is_migrated_once() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join("Relay/project.json");
    let target = dir.path().join("Rivet/project.json");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&legacy, "{\"old\":true}").unwrap();

    assert!(rivet::core::store::migrate_legacy(&target, &legacy));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "{\"old\":true}");
    assert!(legacy.exists(), "old file kept as a backup");

    // Never overwrites an existing Rivet project.
    std::fs::write(&target, "{\"new\":true}").unwrap();
    assert!(!rivet::core::store::migrate_legacy(&target, &legacy));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "{\"new\":true}");
}

#[test]
fn history_lives_in_its_own_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.json");
    let history = store::history_path(&path);
    let mut p = store::seed_project();
    for at in 0..3 {
        store::push_history(&mut p, entry(at));
    }
    store::save(&path, &p).unwrap();
    let project_text = std::fs::read_to_string(&path).unwrap();
    assert!(!project_text.contains("\"history\""), "project.json stays small");
    let history_text = std::fs::read_to_string(&history).unwrap();
    assert!(history_text.contains("\"h2\""));
    assert_eq!(store::load(&path).unwrap().history, p.history);

    // Saving the project again without new history doesn't rewrite history.json.
    let before = std::fs::metadata(&history).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    p.name = "Renamed".into();
    store::save(&path, &p).unwrap();
    assert_eq!(std::fs::metadata(&history).unwrap().modified().unwrap(), before);
    // A new entry does.
    store::push_history(&mut p, entry(9));
    store::save(&path, &p).unwrap();
    assert_eq!(store::load(&path).unwrap().history[0].at, 9);
}

#[test]
fn inline_history_of_older_projects_moves_out() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.json");
    let mut p = store::seed_project();
    store::push_history(&mut p, entry(1));
    // An older project.json with the history inline.
    let mut legacy = serde_json::to_value(&p).unwrap();
    legacy["history"] = serde_json::to_value(&p.history).unwrap();
    std::fs::write(&path, legacy.to_string()).unwrap();
    let loaded = store::load(&path).unwrap();
    assert_eq!(loaded.history, p.history, "inline history still read");
    store::save(&path, &loaded).unwrap();
    assert!(!std::fs::read_to_string(&path).unwrap().contains("\"history\""));
    assert_eq!(store::load(&path).unwrap().history, p.history, "now from history.json");
    assert_eq!(HISTORY_CAP, 10_000);
}
