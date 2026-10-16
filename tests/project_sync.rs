//! The running app merges changes other processes (e.g. `rivet mcp`) make to
//! project.json instead of overwriting them.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use rivet::app::{self, EXTERNAL_NOTICE, Workspace};
use rivet::core::{
    Collection, EnvColor, Environment, HistoryEntry, KeyValue, Method, Project, RequestSpec,
    store,
};
use rivet::theme::Palette;

struct Fixture {
    _dir: tempfile::TempDir,
    store: PathBuf,
    /// Saved request opened in a tab without edits.
    clean_id: String,
    /// Saved request opened in a tab and edited (not saved again).
    dirty_id: String,
}

fn fixture() -> (Fixture, Project) {
    let dir = tempfile::tempdir().unwrap();
    let mut project = store::seed_project();
    let mut dev = Environment::new("Dev", EnvColor::Green);
    dev.vars = vec![
        KeyValue::new("baseUrl", "https://dev.local"),
        KeyValue::new("token", "old"),
    ];
    project.environments = vec![dev];
    let mut mine = Collection::new("Мои запросы");
    let clean = RequestSpec::new("Clean", Method::Get, "https://dev.local/clean");
    let dirty = RequestSpec::new("Dirty", Method::Get, "https://dev.local/dirty");
    let (clean_id, dirty_id) = (clean.id.clone(), dirty.id.clone());
    mine.requests = vec![clean, dirty];
    project.collections = vec![mine];
    let store = dir.path().join("project.json");
    store::save(&store, &project).unwrap();
    (Fixture { _dir: dir, store, clean_id, dirty_id }, project)
}

fn open<'a>(
    cx: &'a mut TestAppContext,
    project: Project,
    store: PathBuf,
) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        Palette::dark().apply(cx);
        app::bind_keys(cx);
    });
    let (view, window) =
        cx.add_window_view(move |window, cx| Workspace::new(project, Some(store), window, cx));
    window.update(|w, cx| w.render_frame(cx));
    (view, window)
}

/// Simulates another process: read, modify, write atomically.
fn edit_externally(path: &Path, f: impl FnOnce(&mut Project)) {
    let mut project = store::load(path).unwrap();
    f(&mut project);
    store::save(path, &project).unwrap();
}

fn advance(window: &mut VisualTestContext, by: Duration) {
    window.executor().advance_clock(by);
    window.run_until_parked();
    window.update(|w, cx| w.render_frame(cx));
}

fn set_var(env: &mut Environment, key: &str, value: &str) {
    match env.vars.iter_mut().find(|v| v.key == key) {
        Some(v) => v.value = value.into(),
        None => env.vars.push(KeyValue::new(key, value)),
    }
}

#[gpui_kit::test]
fn external_changes_are_merged_and_tabs_preserved(cx: &mut TestAppContext) {
    let (fx, project) = fixture();
    let (view, window) = open(cx, project, fx.store.clone());
    view.update_in(window, |v, window, cx| {
        v.open_saved(0, 0, window, cx);
        v.open_saved(0, 1, window, cx);
        // Unsaved edit in the "Dirty" tab.
        v.tabs.last_mut().unwrap().request.url = "https://dev.local/edited".into();
        v.new_tab(window, cx);
        v.tabs.last_mut().unwrap().request.url = "https://scratch.local".into();
        // "Clean" is the active tab.
        v.activate_tab(1, window, cx);
    });
    advance(window, Duration::from_millis(500)); // the app's own save
    view.read_with(window, |v, _| assert_ne!(v.notice.as_deref(), Some(EXTERNAL_NOTICE)));
    let tabs_before = view.read_with(window, |v, _| v.tabs.len());
    window.update(|w, _| assert!(w.try_find(("collection-drop", 1usize)).is_none()));

    let mut ai = Collection::new("AI");
    let ai_request = RequestSpec::new("Ping", Method::Get, "https://ai.local/ping");
    let ai_request_id = ai_request.id.clone();
    ai.requests.push(ai_request.clone());
    let entry_id = rivet::core::new_id();
    edit_externally(&fx.store, |p| {
        p.collections[0].requests[0].url = "https://dev.local/clean-v2".into();
        p.collections[0].requests[1].url = "https://dev.local/dirty-v2".into();
        p.collections.push(ai);
        set_var(&mut p.environments[0], "token", "from-ai");
        p.history.insert(
            0,
            HistoryEntry {
                id: entry_id.clone(),
                at: 1_900_000_000,
                env: "Dev".into(),
                request: ai_request,
                status: Some(200),
                duration_ms: Some(12),
            },
        );
    });
    advance(window, Duration::from_millis(1100));

    view.read_with(window, |v, cx| {
        let p = &v.project;
        assert_eq!(p.collections.len(), 2);
        assert_eq!(p.collections[1].name, "AI");
        assert_eq!(p.collections[1].requests[0].id, ai_request_id);
        assert_eq!(p.environments[0].var("token"), Some("from-ai"));
        assert_eq!(p.history.len(), 1);
        assert_eq!(v.history_panel.read(cx).entries()[0].id, entry_id);
        assert_eq!(v.notice.as_deref(), Some(EXTERNAL_NOTICE));

        // Tabs survive; a clean tab follows the disk, edits are never lost.
        assert_eq!(v.tabs.len(), tabs_before);
        let tab = |id: &str| v.tabs.iter().find(|t| t.request.id == id).unwrap();
        assert_eq!(tab(&fx.clean_id).request.url, "https://dev.local/clean-v2");
        assert_eq!(tab(&fx.dirty_id).request.url, "https://dev.local/edited");
        assert!(v.tabs.iter().any(|t| t.request.url == "https://scratch.local"));
        assert_eq!(v.active().unwrap().id, fx.clean_id);
        assert_eq!(v.url_input.read(cx).value().as_ref(), "https://dev.local/clean-v2");
    });
    // The sidebar shows the new collection and its request.
    window.update(|w, _| {
        assert!(w.try_find(("collection-drop", 1usize)).is_some());
        assert!(w.try_find(("saved-drag", 1000usize)).is_some());
    });
}

#[gpui_kit::test]
fn app_save_keeps_external_changes(cx: &mut TestAppContext) {
    let (fx, project) = fixture();
    let (view, window) = open(cx, project, fx.store.clone());

    // App-side edit; its debounced save is pending...
    view.update(window, |v, cx| {
        set_var(&mut v.project.environments[0], "baseUrl", "https://app.local");
        v.schedule_save(cx);
    });
    // ...when another process writes (before the next poll tick).
    edit_externally(&fx.store, |p| {
        set_var(&mut p.environments[0], "token", "from-ai");
        p.collections.push(Collection::new("AI"));
    });
    advance(window, Duration::from_millis(500));

    let saved = store::load(&fx.store).unwrap();
    let dev = &saved.environments[0];
    assert_eq!(dev.var("baseUrl"), Some("https://app.local"));
    assert_eq!(dev.var("token"), Some("from-ai"));
    assert_eq!(saved.collections.len(), 2);
    view.read_with(window, |v, _| {
        assert_eq!(v.project.collections.len(), 2);
        assert_eq!(v.project.environments[0].var("token"), Some("from-ai"));
        assert_eq!(v.notice.as_deref(), Some(EXTERNAL_NOTICE));
    });

    // Later app edits to a variable changed externally win: the external
    // change was already merged.
    view.update(window, |v, cx| {
        set_var(&mut v.project.environments[0], "token", "from-app");
        v.schedule_save(cx);
    });
    advance(window, Duration::from_secs(2));
    let saved = store::load(&fx.store).unwrap();
    assert_eq!(saved.environments[0].var("token"), Some("from-app"));
    assert_eq!(saved.environments[0].var("baseUrl"), Some("https://app.local"));
    assert_eq!(saved.collections.len(), 2);
}

#[gpui_kit::test]
fn own_saves_do_not_trigger_reload(cx: &mut TestAppContext) {
    let (fx, project) = fixture();
    let (view, window) = open(cx, project, fx.store.clone());
    for round in 0..3 {
        view.update_in(window, |v, window, cx| {
            set_var(&mut v.project.environments[0], "round", &round.to_string());
            v.new_tab(window, cx);
        });
        advance(window, Duration::from_millis(500));
        advance(window, Duration::from_millis(1500));
        view.read_with(window, |v, _| {
            assert_ne!(v.notice.as_deref(), Some(EXTERNAL_NOTICE));
            assert_eq!(v.tabs.len(), round + 2);
        });
        let saved = store::load(&fx.store).unwrap();
        assert_eq!(saved.environments[0].var("round"), Some(round.to_string().as_str()));
        assert_eq!(saved.open_tabs.len(), round + 2);
    }
}

#[gpui_kit::test]
fn externally_deleted_collection_detaches_tabs(cx: &mut TestAppContext) {
    let (fx, project) = fixture();
    let (view, window) = open(cx, project, fx.store.clone());
    let collection_id = view.update_in(window, |v, window, cx| {
        v.open_saved(0, 0, window, cx);
        let id = v.project.collections[0].id.clone();
        v.collapsed.insert(format!("col:{id}"));
        id
    });
    advance(window, Duration::from_millis(500));
    edit_externally(&fx.store, |p| p.collections.clear());
    advance(window, Duration::from_millis(1100));
    view.read_with(window, |v, _| {
        assert!(v.project.collections.is_empty());
        let tab = v.tabs.iter().find(|t| t.request.id == fx.clean_id).unwrap();
        assert_eq!(tab.request.collection_id, None);
        assert!(!v.collapsed.iter().any(|k| k.contains(&collection_id)));
    });
}

#[test]
fn merge_projects_three_way() {
    let mut base = store::seed_project();
    let mut env = Environment::new("Dev", EnvColor::Green);
    env.vars = vec![KeyValue::new("a", "1"), KeyValue::new("b", "1")];
    base.environments = vec![env];
    base.collections = vec![Collection::new("Shared")];

    let mut local = base.clone();
    local.environments[0].vars[0].value = "local".into();
    local.environments[0].vars[1].value = "local".into();
    local.collections.push(Collection::new("Local"));
    local.settings.timeout_ms = 1;

    let mut disk = base.clone();
    disk.environments[0].vars[1].value = "disk".into();
    disk.collections.clear();
    disk.collections.push(Collection::new("Disk"));

    let merged = store::merge_projects(&base, &local, &disk);
    let vars = &merged.environments[0];
    assert_eq!(vars.var("a"), Some("local"));
    // Same variable changed on both sides: disk wins.
    assert_eq!(vars.var("b"), Some("disk"));
    let names: Vec<&str> = merged.collections.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Disk", "Local"]);
    assert_eq!(merged.settings.timeout_ms, 1);
}
