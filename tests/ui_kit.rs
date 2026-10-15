use gpui_kit::test::TestWindowExt;
use gpui_kit::{ElementId, Entity, TestAppContext, VisualTestContext};
use rivet::gallery::Gallery;
use rivet::theme::Palette;

fn open(cx: &mut TestAppContext) -> (Entity<Gallery>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        Palette::dark().apply(cx);
    });
    let (view, window) = cx.add_window_view(|_, _| Gallery::new());
    window.update(|window, cx| window.render_frame(cx));
    (view, window)
}

fn child(parent: impl Into<ElementId>, name: &'static str) -> ElementId {
    ElementId::NamedChild(std::sync::Arc::new(parent.into()), name.into())
}

#[gpui_kit::test]
fn primary_button_clicks_and_disabled_does_not(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| {
        w.click("btn-send", cx);
        w.click("btn-send", cx);
        w.click("btn-disabled", cx);
    });
    assert_eq!(view.read_with(window, |v, _| v.clicks), 2);
}

#[gpui_kit::test]
fn segmented_selects_item(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| w.within("seg-response").click(2usize, cx));
    assert_eq!(view.read_with(window, |v, _| v.segment), 2);
}

#[gpui_kit::test]
fn tabs_activate_close_and_add(cx: &mut TestAppContext) {
    let (view, window) = open(cx);

    window.update(|w, cx| w.click(("tab", 2usize), cx));
    assert_eq!(view.read_with(window, |v, _| v.active_tab), Some(2));

    // Closing the active tab must not also re-activate it via the tab's own click.
    window.update(|w, cx| w.click(child(("tab", 2usize), "close"), cx));
    view.read_with(window, |v, _| {
        assert_eq!(v.tabs.len(), 2);
        assert_eq!(v.active_tab, Some(1));
    });

    window.update(|w, cx| {
        w.render_frame(cx);
        w.click(child("tabbar", "new"), cx);
    });
    view.read_with(window, |v, _| {
        assert_eq!(v.tabs.len(), 3);
        assert_eq!(v.active_tab, Some(2));
    });
}

#[gpui_kit::test]
fn tree_folder_collapses_and_row_selects(cx: &mut TestAppContext) {
    let (view, window) = open(cx);

    window.update(|w, cx| w.click(("tree-row", 3usize), cx));
    assert_eq!(view.read_with(window, |v, _| v.selected_row), Some(3));

    window.update(|w, cx| {
        w.click("folder-orders", cx);
        w.render_frame(cx);
        assert!(
            w.try_find(("tree-row", 0usize)).is_none(),
            "rows hidden when folder collapsed"
        );
    });
    assert!(!view.read_with(window, |v, _| v.folder_open));
}

#[gpui_kit::test]
fn gallery_kv_table_toggles_and_adds(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| {
        assert!(w.try_find("url-field").is_some());
        w.within("kv-table").click(("toggle", 2usize), cx);
    });
    assert!(view.read_with(window, |v, _| v.kv_rows[2].enabled));
    window.update(|w, cx| {
        w.render_frame(cx);
        w.within("kv-table").click("add", cx);
    });
    assert_eq!(view.read_with(window, |v, _| v.kv_rows.len()), 4);
}
