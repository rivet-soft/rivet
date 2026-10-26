//! Settings → MCP: permission toggles, snippet copy, one-click install into a
//! temp client config, activity log.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, ScrollDelta, TestAppContext, VisualTestContext, point, px};
use rivet::app::settings_panel::{SettingsPanel, SettingsPanelEvent};
use rivet::core::mcp_activity::{self, ActivityEntry};
use rivet::core::mcp_clients::{McpClientPaths, RivetCommand, backup_path};
use rivet::core::{McpSettings, Settings};
use rivet::theme::Palette;

struct Fixture {
    dir: tempfile::TempDir,
    paths: McpClientPaths,
    project: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let paths = McpClientPaths {
        claude_desktop: dir.path().join("Claude").join("claude_desktop_config.json"),
        cursor: dir.path().join(".cursor").join("mcp.json"),
        claude_code_config: dir.path().join(".claude.json"),
        claude_cli: Some(dir.path().join("no-such-claude")),
    };
    let project = dir.path().join("project.json");
    Fixture { dir, paths, project }
}

fn command() -> RivetCommand {
    RivetCommand { exe: PathBuf::from("/Applications/Rivet.app/Contents/MacOS/Rivet"), args: vec!["mcp".into()] }
}

fn open<'a>(
    cx: &'a mut TestAppContext,
    fx: &Fixture,
) -> (Entity<SettingsPanel>, Rc<RefCell<Vec<Settings>>>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        Palette::dark().apply(cx);
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let ev = events.clone();
    let (paths, project) = (fx.paths.clone(), fx.project.clone());
    let (view, window) = cx.add_window_view(move |window, cx| {
        let mut panel = SettingsPanel::new(Settings::default(), "test".into(), window, cx);
        panel.set_command(command(), cx);
        panel.set_client_paths(paths, cx);
        panel.set_project_path(Some(project), cx);
        let ev = ev.clone();
        cx.subscribe_self(move |_, e: &SettingsPanelEvent, _| {
            let SettingsPanelEvent::Changed(s) = e;
            ev.borrow_mut().push(s.clone());
        })
        .detach();
        panel
    });
    window.run_until_parked();
    // The MCP section is below the fold: scroll the panel to the bottom.
    window.update(|w, cx| {
        w.render_frame(cx);
        w.scroll("settings-panel", ScrollDelta::Pixels(point(px(0.), px(-5000.))), cx);
        w.render_frame(cx);
    });
    (view, events, window)
}

#[gpui_kit::test]
fn toggles_change_mcp_settings(cx: &mut TestAppContext) {
    let fx = fixture();
    let (view, events, window) = open(cx, &fx);
    assert_eq!(view.read_with(window, |v, _| v.settings().mcp), McpSettings::default());

    window.update(|w, cx| {
        click_in_view(w, cx, "settings-mcp-readonly");
        w.render_frame(cx);
        click_in_view(w, cx, "settings-mcp-send");
        w.render_frame(cx);
        assert!(w.try_find("settings-mcp-secrets-warning").is_none());
        click_in_view(w, cx, "settings-mcp-secrets");
        w.render_frame(cx);
        assert!(w.try_find("settings-mcp-secrets-warning").is_some());
        for id in ["settings-mcp-mocks", "settings-mcp-files", "settings-mcp-history"] {
            click_in_view(w, cx, id);
            w.render_frame(cx);
        }
        click_in_view(w, cx, "settings-mcp-enabled");
        w.render_frame(cx);
    });
    let expected = McpSettings {
        enabled: false,
        read_only: true,
        allow_send: false,
        reveal_secrets: true,
        allow_mocks: false,
        allow_files: false,
        allow_history: false,
    };
    assert_eq!(view.read_with(window, |v, _| v.settings().mcp), expected);
    let events = events.borrow();
    assert_eq!(events.len(), 7);
    assert!(events[0].mcp.read_only && events[0].mcp.allow_send);
    assert_eq!(events[6].mcp, expected);

    // While MCP is off the other toggles are disabled.
    window.update(|w, cx| {
        // The panel is taller than the test window: bring the MCP toggles in.
        for _ in 0..60 {
            let y = w.find("settings-mcp-readonly").bounds().origin.y.as_f32();
            let h = w.find("settings-mcp-history").bounds().origin.y.as_f32();
            let view_h = w.viewport_size().height.as_f32();
            if y > 0. && h + 40. < view_h {
                break;
            }
            let dy = if h + 40. >= view_h { -60. } else { 60. };
            w.scroll("settings-panel", ScrollDelta::Pixels(point(px(0.), px(dy))), cx);
        }
        w.click("settings-mcp-readonly", cx);
        w.render_frame(cx);
    });
    assert!(view.read_with(window, |v, _| v.settings().mcp.read_only));
}

#[gpui_kit::test]
fn copy_puts_snippets_on_clipboard(cx: &mut TestAppContext) {
    let fx = fixture();
    let (view, _, window) = open(cx, &fx);
    window.update(|w, cx| {
        w.click(("mcp-copy", 0usize), cx);
        w.render_frame(cx);
    });
    let clip = window.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
    assert_eq!(clip.as_deref(), Some("claude mcp add rivet -- \"/Applications/Rivet.app/Contents/MacOS/Rivet\" mcp"));
    assert_eq!(view.read_with(window, |v, _| v.notice.clone()).as_deref(), Some("Скопировано"));

    window.update(|w, cx| {
        w.click(("mcp-copy", 2usize), cx);
        w.render_frame(cx);
    });
    let clip = window.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text())).unwrap();
    let json: serde_json::Value = serde_json::from_str(&clip).unwrap();
    assert_eq!(json["mcpServers"]["rivet"]["command"], "/Applications/Rivet.app/Contents/MacOS/Rivet");
    assert_eq!(json["mcpServers"]["rivet"]["args"][0], "mcp");
}

#[gpui_kit::test]
fn install_writes_config_after_confirmation(cx: &mut TestAppContext) {
    let fx = fixture();
    std::fs::create_dir_all(fx.paths.cursor.parent().unwrap()).unwrap();
    std::fs::write(&fx.paths.cursor, r#"{"mcpServers":{"other":{"command":"x"}}}"#).unwrap();
    let (view, _, window) = open(cx, &fx);
    assert!(!view.read_with(window, |v, _| v.mcp_status.cursor));

    // Cancel: nothing is written.
    window.update(|w, cx| {
        w.click(("mcp-install", 1usize), cx);
        w.render_frame(cx);
        assert!(w.try_find("confirm").is_some());
        w.click("cancel", cx);
        w.render_frame(cx);
        assert!(w.try_find("confirm").is_none());
    });
    window.run_until_parked();
    assert!(!fx.paths.claude_desktop.exists());

    // Claude Desktop: config created.
    window.update(|w, cx| {
        w.click(("mcp-install", 1usize), cx);
        w.render_frame(cx);
        w.click("confirm", cx);
        w.render_frame(cx);
    });
    window.run_until_parked();
    let desktop: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&fx.paths.claude_desktop).unwrap()).unwrap();
    assert_eq!(desktop["mcpServers"]["rivet"]["args"][0], "mcp");
    view.read_with(window, |v, _| {
        assert!(v.mcp_status.claude_desktop);
        assert!(v.installing.is_none());
        assert!(v.notice.as_deref().unwrap().contains("Claude Desktop"));
    });

    // Cursor: merged, other servers kept, backup written.
    window.update(|w, cx| {
        w.click(("mcp-install", 2usize), cx);
        w.render_frame(cx);
        w.click("confirm", cx);
        w.render_frame(cx);
    });
    window.run_until_parked();
    let cursor: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&fx.paths.cursor).unwrap()).unwrap();
    assert_eq!(cursor["mcpServers"]["other"]["command"], "x");
    assert!(cursor["mcpServers"]["rivet"].is_object());
    assert!(backup_path(&fx.paths.cursor).exists());
    assert!(view.read_with(window, |v, _| v.mcp_status.cursor));

    // Claude Code without a CLI: a readable error, nothing else happens.
    window.update(|w, cx| {
        w.click(("mcp-install", 0usize), cx);
        w.render_frame(cx);
        w.click("confirm", cx);
        w.render_frame(cx);
    });
    window.run_until_parked();
    view.read_with(window, |v, _| {
        assert!(v.notice.as_deref().unwrap().contains("ошибка"), "{:?}", v.notice);
        assert!(!v.mcp_status.claude_code);
    });
    assert!(fx.dir.path().exists());
}

#[gpui_kit::test]
fn activity_is_listed_and_cleared(cx: &mut TestAppContext) {
    let fx = fixture();
    let log = mcp_activity::activity_path(&fx.project);
    mcp_activity::append(&log, &ActivityEntry::new("send_request", true, "GET https://x → 200", Some("claude-code".into()))).unwrap();
    mcp_activity::append(&log, &ActivityEntry::new("save_request", false, "read-only", None)).unwrap();
    let (view, _, window) = open(cx, &fx);
    view.read_with(window, |v, _| {
        assert_eq!(v.activity.len(), 2);
        assert_eq!(v.activity[0].tool, "save_request");
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find(("mcp-activity", 1usize)).is_some());
    });

    // Polling picks new lines up while the panel is visible.
    view.update(window, |v, cx| v.set_visible(true, cx));
    mcp_activity::append(&log, &ActivityEntry::new("list_collections", true, "коллекций: 1", None)).unwrap();
    window.executor().advance_clock(std::time::Duration::from_secs(3));
    window.run_until_parked();
    assert_eq!(view.read_with(window, |v, _| v.activity.len()), 3);

    window.update(|w, cx| {
        w.render_frame(cx);
        w.click("mcp-clear-activity", cx);
        w.render_frame(cx);
    });
    assert!(view.read_with(window, |v, _| v.activity.is_empty()));
    assert!(mcp_activity::read(&log, 10).is_empty());
    view.update(window, |v, cx| v.set_visible(false, cx));
}

/// Scrolls the settings panel until `id` is on screen, then clicks it (the
/// panel is taller than the test window).
fn click_in_view(w: &mut gpui_kit::Window, cx: &mut gpui_kit::App, id: &'static str) {
    for _ in 0..80 {
        let y = w.find(id).bounds().origin.y.as_f32();
        let view_h = w.viewport_size().height.as_f32();
        if y > 10. && y + 40. < view_h {
            break;
        }
        let dy = if y + 40. >= view_h { -50. } else { 50. };
        w.scroll("settings-panel", ScrollDelta::Pixels(point(px(0.), px(dy))), cx);
    }
    w.click(id, cx);
}
