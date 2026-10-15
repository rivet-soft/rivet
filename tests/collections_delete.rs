use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use rivet::app::{self, Workspace};
use rivet::core::{Collection, Method, Project, RequestSpec, store};
use rivet::model::RailMode;
use rivet::theme::Palette;

struct Fixture {
    _dir: tempfile::TempDir,
    store: PathBuf,
    source: PathBuf,
    source_bytes: Vec<u8>,
    linked_id: String,
    manual_id: String,
}

/// Two collections: "Shop" linked to a spec file on disk (with one request) and a
/// manual "Team" collection.
fn fixture() -> (Fixture, Project) {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("shop-api.yaml");
    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/examples/shop-api.yaml"),
        &source,
    )
    .unwrap();
    let source_bytes = std::fs::read(&source).unwrap();

    let mut linked = Collection::new("Shop");
    linked.source = Some(source.clone());
    linked.folders.push("Orders".into());
    let mut req = RequestSpec::new("Orders", Method::Get, "https://example.com/orders");
    req.collection_id = Some(linked.id.clone());
    req.folder = Some("Orders".into());
    linked.requests.push(req);
    let manual = Collection::new("Team");

    let mut project = store::seed_project();
    let linked_id = linked.id.clone();
    let manual_id = manual.id.clone();
    project.collections = vec![linked, manual];
    let store = dir.path().join("project.json");
    store::save(&store, &project).unwrap();
    (
        Fixture {
            _dir: dir,
            store,
            source,
            source_bytes,
            linked_id,
            manual_id,
        },
        project,
    )
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

/// Opens the linked collection's request in a tab and shows the Collections rail.
fn open_tab_and_panel(view: &Entity<Workspace>, window: &mut VisualTestContext) {
    view.update_in(window, |v, window, cx| {
        v.open_saved(0, 0, window, cx);
        v.set_rail(RailMode::Collections, window, cx);
    });
    window.update(|w, cx| w.render_frame(cx));
}

fn flush_save(cx: &mut TestAppContext) {
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn panel_delete_removes_collection_but_keeps_source_file(cx: &mut TestAppContext) {
    let (fx, project) = fixture();
    let (view, window) = open(cx, project, fx.store.clone());
    open_tab_and_panel(&view, window);
    view.update(window, |v, _| {
        v.collapsed.insert(format!("col:{}", fx.linked_id));
        v.collapsed
            .insert(format!("col:{}:folder:Orders", fx.linked_id));
    });

    window.update(|w, cx| {
        w.click("collection-delete", cx);
        w.render_frame(cx);
        assert!(w.try_find("confirm").is_some());
        w.click("confirm", cx);
        w.render_frame(cx);
    });

    view.read_with(window, |v, cx| {
        assert_eq!(v.project.collections.len(), 1);
        assert_eq!(v.project.collections[0].id, fx.manual_id);
        // The tab survives, detached from the deleted collection.
        let tab = v
            .tabs
            .iter()
            .find(|t| t.request.name == "Orders")
            .expect("tab stays open");
        assert_eq!(tab.request.collection_id, None);
        assert_eq!(tab.request.folder, None);
        assert!(!v.collapsed.iter().any(|k| k.contains(&fx.linked_id)));
        assert_eq!(v.notice.as_deref(), Some("Коллекция «Shop» удалена"));
        assert!(!v.collection_panel.read(cx).confirm_delete);
    });
    // The imported file is never touched.
    assert_eq!(std::fs::read(&fx.source).unwrap(), fx.source_bytes);

    flush_save(cx);
    let saved = store::load(&fx.store).unwrap();
    assert_eq!(saved.collections.len(), 1);
    assert_eq!(saved.collections[0].id, fx.manual_id);
    assert!(
        !std::fs::read_to_string(&fx.store)
            .unwrap()
            .contains(&fx.linked_id)
    );
    assert!(fx.source.exists());
}

#[gpui_kit::test]
fn panel_delete_cancel_keeps_collection(cx: &mut TestAppContext) {
    let (fx, project) = fixture();
    let (view, window) = open(cx, project, fx.store.clone());
    open_tab_and_panel(&view, window);

    window.update(|w, cx| {
        w.click("collection-delete", cx);
        w.render_frame(cx);
        w.click("cancel", cx);
        w.render_frame(cx);
        assert!(w.try_find("confirm").is_none());
    });
    view.read_with(window, |v, cx| {
        assert_eq!(v.project.collections.len(), 2);
        assert!(!v.collection_panel.read(cx).confirm_delete);
        let tab = v.tabs.iter().find(|t| t.request.name == "Orders").unwrap();
        assert_eq!(
            tab.request.collection_id.as_deref(),
            Some(fx.linked_id.as_str())
        );
    });
    flush_save(cx);
    assert_eq!(store::load(&fx.store).unwrap().collections.len(), 2);
    assert_eq!(std::fs::read(&fx.source).unwrap(), fx.source_bytes);
}

#[gpui_kit::test]
fn sidebar_menu_deletes_collection_after_confirmation(cx: &mut TestAppContext) {
    let (fx, project) = fixture();
    let (view, window) = open(cx, project, fx.store.clone());

    // Cancel first: nothing changes.
    window.update(|w, cx| {
        w.click("sidebar-menu-trigger-collection-0", cx);
        w.render_frame(cx);
        w.click(("sidebar-collection-delete", 0usize), cx);
        w.render_frame(cx);
        assert!(w.try_find("confirm").is_some());
        w.click("cancel", cx);
        w.render_frame(cx);
    });
    assert_eq!(
        view.read_with(window, |v, _| v.project.collections.len()),
        2
    );

    window.update(|w, cx| {
        w.click("sidebar-menu-trigger-collection-0", cx);
        w.render_frame(cx);
        w.click(("sidebar-collection-delete", 0usize), cx);
        w.render_frame(cx);
        w.click("confirm", cx);
        w.render_frame(cx);
        assert!(w.try_find(("collection", 1usize)).is_none());
    });
    view.read_with(window, |v, _| {
        assert_eq!(v.project.collections.len(), 1);
        assert_eq!(v.project.collections[0].id, fx.manual_id);
    });
    assert_eq!(std::fs::read(&fx.source).unwrap(), fx.source_bytes);
    flush_save(cx);
    let saved = store::load(&fx.store).unwrap();
    assert_eq!(saved.collections.len(), 1);
    assert_eq!(saved.collections[0].id, fx.manual_id);
}
