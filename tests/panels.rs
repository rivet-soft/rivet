use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use rivet::app::env_panel::{EnvPanel, EnvPanelEvent};
use rivet::app::history_panel::{
    HistoryPanel, HistoryPanelEvent, day_label, format_hm, status_style,
};
use rivet::core::types::{EnvColor, Environment, HistoryEntry, RequestSpec};
use rivet::model::Method;
use rivet::theme::Palette;

fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        Palette::dark().apply(cx);
    });
}

fn open_env(
    cx: &mut TestAppContext,
) -> (
    Entity<EnvPanel>,
    Rc<RefCell<Vec<EnvPanelEvent>>>,
    &mut VisualTestContext,
) {
    init(cx);
    let events = Rc::new(RefCell::new(Vec::new()));
    let ev = events.clone();
    let (view, window) = cx.add_window_view(move |window, cx| {
        let envs = vec![
            Environment::new("Dev", EnvColor::Green),
            Environment::new("Prod", EnvColor::Red),
            Environment::new("Local mock", EnvColor::Amber),
        ];
        let panel = EnvPanel::new(envs, 0, window, cx);
        let ev = ev.clone();
        cx.subscribe_self(move |_, e: &EnvPanelEvent, _| ev.borrow_mut().push(e.clone()))
            .detach();
        panel
    });
    window.update(|w, cx| w.render_frame(cx));
    (view, events, window)
}

#[gpui_kit::test]
fn env_panel_add_env_and_edit_var(cx: &mut TestAppContext) {
    let (view, events, window) = open_env(cx);

    window.update(|w, cx| w.click("env-add", cx));
    view.read_with(window, |v, _| {
        assert_eq!(v.envs().len(), 4);
        assert_eq!(v.selected(), 3);
        assert_eq!(v.envs()[3].vars[0].key, "baseUrl");
    });
    assert!(events.borrow().contains(&EnvPanelEvent::Changed));

    window.update(|w, cx| {
        w.render_frame(cx);
        w.click(("var-value", 0usize), cx);
        w.input("http://x.test", cx);
    });
    // InputState events reach the panel after the update flushes.
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |v, _| {
        assert_eq!(v.envs()[3].var("baseUrl"), Some("http://x.test"))
    });

    // Rename via the name input.
    window.update(|w, cx| {
        w.click("env-name", cx);
        w.press("cmd-a", cx);
        w.input("Staging", cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |v, _| assert_eq!(v.envs()[3].name, "Staging"));

    // Add a variable, type its key, toggle it off, then delete it.
    window.update(|w, cx| {
        w.click("var-add", cx);
        w.render_frame(cx);
        w.click(("var-key", 1usize), cx);
        w.input("apiToken", cx);
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(
            w.try_find(("var-eye", 1usize)).is_some(),
            "secret key gets an eye toggle"
        );
        w.click(("var-toggle", 1usize), cx);
    });
    view.read_with(window, |v, _| {
        let kv = &v.envs()[3].vars[1];
        assert_eq!(kv.key, "apiToken");
        assert!(!kv.enabled);
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        w.click(("var-delete", 1usize), cx);
    });
    assert_eq!(view.read_with(window, |v, _| v.envs()[3].vars.len()), 1);
}

#[gpui_kit::test]
fn env_panel_select_color_production_and_activate(cx: &mut TestAppContext) {
    let (view, events, window) = open_env(cx);
    window.update(|w, cx| {
        assert!(
            w.try_find("env-activate").is_none(),
            "active env shows no activate button"
        );
        w.click(("env-item", 2usize), cx);
        w.render_frame(cx);
        w.click(("env-color", 4usize), cx);
        w.click("env-production", cx);
        w.click("env-activate", cx);
    });
    view.read_with(window, |v, _| {
        let env = &v.envs()[2];
        assert_eq!(env.color, rivet::core::types::EnvColor::Purple);
        assert!(env.production);
        assert_eq!(v.active(), 2);
    });
    assert_eq!(events.borrow().last(), Some(&EnvPanelEvent::Activated(2)));

    // Switching env rebuilds rows with the new values.
    window.update(|w, cx| {
        w.click(("env-item", 0usize), cx);
        w.render_frame(cx);
    });
    view.read_with(window, |v, _| assert_eq!(v.selected(), 0));

    // Duplicate then delete.
    window.update(|w, cx| w.click("env-duplicate", cx));
    view.read_with(window, |v, _| {
        assert_eq!(v.envs().len(), 4);
        assert_eq!(v.envs()[1].name, "Dev (копия)");
        assert_ne!(v.envs()[1].id, v.envs()[0].id);
        assert_eq!(v.active(), 3, "active index follows its env");
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        w.click("env-delete", cx);
    });
    view.read_with(window, |v, _| {
        assert_eq!(v.envs().len(), 3);
        assert_eq!(v.active(), 2);
    });
}

fn entry(name: &str, method: Method, url: &str, at: u64, status: Option<u16>) -> HistoryEntry {
    HistoryEntry {
        id: name.into(),
        at,
        env: "Dev".into(),
        request: RequestSpec::new(name, method, url),
        status,
        duration_ms: Some(148),
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[gpui_kit::test]
fn history_panel_rows_filter_and_open(cx: &mut TestAppContext) {
    init(cx);
    let events = Rc::new(RefCell::new(Vec::new()));
    let ev = events.clone();
    let (view, window) = cx.add_window_view(move |window, cx| {
        let ev = ev.clone();
        cx.subscribe_self(move |_, e: &HistoryPanelEvent, _| ev.borrow_mut().push(e.clone()))
            .detach();
        HistoryPanel::new(window, cx)
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("history-empty").is_some());
    });

    let t = now();
    let entries = vec![
        entry(
            "Old order",
            Method::Delete,
            "{{baseUrl}}/orders/1",
            t - 3 * 86_400,
            Some(500),
        ),
        entry(
            "List orders",
            Method::Get,
            "{{baseUrl}}/orders",
            t,
            Some(200),
        ),
        entry(
            "Login",
            Method::Post,
            "{{baseUrl}}/auth/login",
            t - 60,
            None,
        ),
    ];
    view.update(window, |v, cx| v.set_entries(entries, cx));
    window.update(|w, cx| {
        w.render_frame(cx);
        for ix in 0..3usize {
            assert!(
                w.try_find(("history-row", ix)).is_some(),
                "row {ix} rendered"
            );
        }
    });
    view.read_with(window, |v, _| {
        let names: Vec<_> = v
            .entries()
            .iter()
            .map(|e| e.request.name.as_str())
            .collect();
        assert_eq!(names, ["List orders", "Login", "Old order"], "newest first");
    });

    window.update(|w, cx| {
        w.click("history-filter", cx);
        w.input("login", cx);
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find(("history-row", 0usize)).is_none());
        assert!(w.try_find(("history-row", 1usize)).is_some());
        assert!(w.try_find(("history-row", 2usize)).is_none());
        w.click(("history-row", 1usize), cx);
    });
    match events.borrow().last() {
        Some(HistoryPanelEvent::Open(e)) => assert_eq!(e.request.name, "Login"),
        other => panic!("expected Open, got {other:?}"),
    }

    window.update(|w, cx| w.click("history-clear", cx));
    assert_eq!(events.borrow().last(), Some(&HistoryPanelEvent::Clear));
}

#[gpui_kit::test]
fn history_panel_virtualizes_500(cx: &mut TestAppContext) {
    init(cx);
    let (view, window) = cx.add_window_view(|window, cx| HistoryPanel::new(window, cx));
    let t = now();
    let entries = (0..500u64)
        .map(|i| entry(&format!("r{i}"), Method::Get, "/x", t - i * 60, Some(200)))
        .collect();
    view.update(window, |v, cx| v.set_entries(entries, cx));
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find(("history-row", 0usize)).is_some());
        assert!(
            w.try_find(("history-row", 499usize)).is_none(),
            "off-screen rows not rendered"
        );
    });
}

#[test]
fn history_formatting() {
    let day = 86_400;
    let now = 20_000 * day + 10 * 3600;
    assert_eq!(day_label(now - 3600, now), "Сегодня");
    assert_eq!(day_label(now - day, now), "Вчера");
    // 1970-01-01 + 19_998 days = 2024-10-02
    assert_eq!(day_label(19_998 * day, now), "02.10.2024");
    assert_eq!(format_hm(19_998 * day + 9 * 3600 + 5 * 60 + 59), "09:05");
    let p = Palette::dark();
    assert_eq!(status_style(None, &p).0, "ERR");
    assert_eq!(status_style(Some(404), &p), ("404".into(), p.warn));
    assert_eq!(status_style(Some(201), &p).1, p.online);
}
