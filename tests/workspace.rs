use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use gpui_kit::test::TestWindowExt;
use gpui_kit::{ElementId, Entity, TestAppContext, VisualTestContext, point, px};
use rivet::app::response_view::ResponseState;
use rivet::app::{self, Workspace};
use rivet::core::{BodyKind, Collection, EnvColor, Environment, KeyValue, Method, Project, store};
use rivet::model::RailMode;
use rivet::theme::{Mode, Palette};
use rivet::ui::TabAction;

fn project() -> Project {
    let mut p = store::seed_project();
    p.spec_path = Some(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/shop-api.yaml").into());
    let mut dev = Environment::new("Dev", EnvColor::Green);
    dev.vars = vec![
        KeyValue::new("baseUrl", "https://dev.shop.local/api"),
        KeyValue::new("customerId", "c_51b2e7a0"),
        KeyValue::new("token", "demo"),
    ];
    let mut prod = Environment::new("Prod", EnvColor::Red);
    prod.production = true;
    prod.vars = vec![KeyValue::new("baseUrl", "https://api.shop.com/v2")];
    let mut mock = Environment::new("Local mock", EnvColor::Amber);
    mock.vars = vec![KeyValue::new("baseUrl", "http://localhost:4010")];
    p.environments = vec![dev, prod, mock];
    p.collections.push(Collection::new("Мои запросы"));
    p
}

fn open_with(
    cx: &mut TestAppContext,
    project: Project,
) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        Palette::dark().apply(cx);
        app::bind_keys(cx);
    });
    let (view, window) =
        cx.add_window_view(move |window, cx| Workspace::new(project, None, window, cx));
    window.update(|w, cx| w.render_frame(cx));
    (view, window)
}

fn open(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    open_with(cx, project())
}

fn child(parent: impl Into<ElementId>, name: &'static str) -> ElementId {
    ElementId::NamedChild(std::sync::Arc::new(parent.into()), name.into())
}

#[gpui_kit::test]
fn first_launch_has_no_demo_spec_or_collections(cx: &mut TestAppContext) {
    let (view, window) = open_with(cx, store::seed_project());
    view.read_with(window, |workspace, _| {
        assert!(workspace.spec_groups().is_empty());
        assert!(workspace.project.collections.is_empty());
        assert!(workspace.project.environments.is_empty());
        assert_eq!(
            workspace.active().map(|request| request.url.as_str()),
            Some("")
        );
    });
}

#[gpui_kit::test]
fn single_request_tab_fills_tab_bar_height_and_keeps_compact_width(cx: &mut TestAppContext) {
    let (_, window) = open(cx);
    window.update(|w, cx| {
        w.render_frame(cx);
        let bar = w.find("tabbar").bounds();
        let tab = w.find(("tab", 0usize)).bounds();
        assert!(tab.size.height.as_f32() >= bar.size.height.as_f32() - 2.);
        assert!(tab.size.width.as_f32() <= 220.);
    });
}

#[gpui_kit::test]
fn copy_curl_button_copies_request_and_shows_confirmation(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    let clipboard = window.update(|w, cx| {
        w.click("copy-curl", cx);
        w.render_frame(cx);
        assert!(w.try_find("notice").is_some());
        cx.read_from_clipboard().and_then(|item| item.text())
    });
    let curl = clipboard.expect("cURL should be on the clipboard");
    assert!(curl.starts_with("curl "));
    assert!(curl.contains("/orders"));
    view.read_with(window, |v, _| {
        assert_eq!(v.notice.as_deref(), Some("cURL скопирован"));
    });
}

#[gpui_kit::test]
fn tab_actions_open_from_right_click_and_ellipsis(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| {
        w.right_click(("tab", 0usize), cx);
        w.render_frame(cx);
        assert!(
            w.try_find(("tab-action", TabAction::NewRequest as usize))
                .is_some()
        );
        w.click(("tab-action", TabAction::Duplicate as usize), cx);
        w.render_frame(cx);
    });
    view.read_with(window, |v, _| {
        assert_eq!(v.tabs.len(), 2);
        assert_eq!(v.active_tab, Some(1));
        assert_ne!(v.tabs[0].request.id, v.tabs[1].request.id);
    });

    window.update(|w, cx| {
        w.click(child(("tab", 1usize), "menu"), cx);
        w.render_frame(cx);
        w.click(("tab-action", TabAction::CloseOthers as usize), cx);
        w.render_frame(cx);
    });
    view.read_with(window, |v, _| {
        assert_eq!(v.tabs.len(), 1);
        assert_eq!(v.active_tab, Some(0));
    });

    window.update(|w, cx| {
        w.click(child(("tab", 0usize), "menu"), cx);
        w.render_frame(cx);
        w.click(("tab-action", TabAction::CloseAll as usize), cx);
        w.render_frame(cx);
        assert!(w.try_find("empty-new-tab").is_some());
    });
    assert!(view.read_with(window, |v, _| v.tabs.is_empty()));
}

#[gpui_kit::test]
fn force_close_cancels_running_tab_while_normal_close_keeps_it(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| {
        w.click("send", cx);
        assert!(view.read(cx).tabs[0].is_sending());
        w.dispatch_action(Box::new(app::CloseTab), cx);
        assert_eq!(view.read(cx).tabs.len(), 1);
        w.dispatch_action(Box::new(app::ForceCloseTab), cx);
    });
    view.read_with(window, |v, _| assert!(v.tabs.is_empty()));
}

#[gpui_kit::test]
fn request_response_divider_resizes_and_respects_minimum_widths(cx: &mut TestAppContext) {
    let (_, window) = open(cx);
    window.update(|w, cx| {
        let divider = w.find("request-response-divider").bounds();
        let initial = w.find("request-editor-pane").bounds().size.width.as_f32();
        let from = divider.center();
        w.drag(from, from + point(px(100.), px(0.)), cx);
        w.render_frame(cx);
        let expanded = w.find("request-editor-pane").bounds().size.width.as_f32();
        assert!(
            expanded > initial + 90.,
            "request pane should follow the divider"
        );

        let from = w.find("request-response-divider").bounds().center();
        w.drag(from, from - point(px(1000.), px(0.)), cx);
        w.render_frame(cx);
        let left = w.find("request-editor-pane").bounds().size.width.as_f32();
        assert!((left - 280.).abs() < 2., "request pane must stop at 280 px");

        let from = w.find("request-response-divider").bounds().center();
        w.drag(from, from + point(px(1200.), px(0.)), cx);
        w.render_frame(cx);
        let right = w.find("response-pane").bounds().size.width.as_f32();
        assert!(
            (right - 280.).abs() < 2.,
            "response pane must stop at 280 px"
        );
    });
}

#[gpui_kit::test]
fn narrow_request_pane_keeps_header_and_body_actions_visible(cx: &mut TestAppContext) {
    let (_, window) = open(cx);
    window.update(|w, cx| {
        let from = w.find("request-response-divider").bounds().center();
        w.drag(from, from - point(px(1000.), px(0.)), cx);
        w.render_frame(cx);
        w.render_frame(cx);
        let pane = w.find("request-editor-pane").bounds();
        let left = pane.origin.x.as_f32();
        let right = left + pane.size.width.as_f32();
        for id in ["request-save", "generate-body", "body-mode"] {
            let bounds = w.find(id).bounds();
            assert!(
                bounds.origin.x.as_f32() >= left - 1.,
                "{id} begins outside request pane"
            );
            assert!(
                bounds.origin.x.as_f32() + bounds.size.width.as_f32() <= right + 1.,
                "{id} extends outside request pane"
            );
        }
        for index in 0..6usize {
            let bounds = w.within("body-mode").find(index).bounds();
            assert!(
                bounds.origin.x.as_f32() + bounds.size.width.as_f32() <= right + 1.,
                "body format {index} is clipped"
            );
            let generate = w.find("generate-body").bounds();
            let overlap_x = bounds.origin.x.as_f32()
                < generate.origin.x.as_f32() + generate.size.width.as_f32()
                && generate.origin.x.as_f32()
                    < bounds.origin.x.as_f32() + bounds.size.width.as_f32();
            let overlap_y = bounds.origin.y.as_f32()
                < generate.origin.y.as_f32() + generate.size.height.as_f32()
                && generate.origin.y.as_f32()
                    < bounds.origin.y.as_f32() + bounds.size.height.as_f32();
            assert!(
                !(overlap_x && overlap_y),
                "Generate overlaps body format {index}"
            );
        }
        let settings_tab = w.find(("subtab", 4usize)).bounds();
        assert!(settings_tab.origin.x.as_f32() + settings_tab.size.width.as_f32() <= right + 1.);
    });
}

#[gpui_kit::test]
fn generate_from_upload_schema_selects_form_data_and_fields(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("upload.yaml");
    std::fs::write(
        &path,
        r#"
openapi: 3.0.3
info: {title: Upload, version: '1'}
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                file: {type: string, format: binary}
                caption: {type: string, example: hello}
      responses: {'200': {description: OK}}
"#,
    )
    .unwrap();
    let mut project = project();
    project.spec_path = Some(path);
    let (view, window) = open_with(cx, project);
    view.read_with(window, |workspace, _| {
        assert_eq!(workspace.active().unwrap().body.kind, BodyKind::None);
    });
    window.update(|w, cx| {
        w.click("generate-body", cx);
        w.render_frame(cx);
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        assert_eq!(
            view.read(cx).active().unwrap().body.kind,
            BodyKind::Multipart
        );
        assert_eq!(
            view.read(cx).editor.read(cx).spec().body.kind,
            BodyKind::Multipart
        );
        assert!(w.try_find("body-mode").is_some());
        assert!(w.within("req-form").try_find(("key", 0usize)).is_some());
    });
    view.read_with(window, |workspace, _| {
        let request = workspace.active().unwrap();
        assert_eq!(request.body.kind, BodyKind::Multipart);
        assert!(
            request
                .body
                .form
                .iter()
                .any(|field| field.key == "file" && field.file)
        );
    });
}

#[gpui_kit::test]
fn new_request_menu_action_opens_requests_from_environments(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| {
        w.press("cmd-2", cx);
        w.render_frame(cx);
        w.dispatch_action(Box::new(app::NewTab), cx);
        w.render_frame(cx);
    });
    view.read_with(window, |workspace, _| {
        assert!(workspace.rail == RailMode::Requests);
        assert_eq!(workspace.tabs.len(), 2);
    });
    window.update(|w, _| assert!(w.try_find("request-response-divider").is_some()));
}

#[gpui_kit::test]
fn reload_openapi_menu_action_refreshes_the_spec(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("menu-openapi.yaml");
    std::fs::write(
        &path,
        "openapi: 3.0.3\ninfo: {title: Before, version: 1}\npaths: {}\n",
    )
    .unwrap();
    let mut project = project();
    project.spec_path = Some(path.clone());
    let (view, window) = open_with(cx, project);
    std::fs::write(
        &path,
        "openapi: 3.0.3\ninfo: {title: After, version: 2}\npaths:\n  /added:\n    get:\n      responses:\n        '200': {description: OK}\n",
    )
    .unwrap();
    window.update(|w, cx| {
        w.dispatch_action(Box::new(app::ReloadSpec), cx);
        w.render_frame(cx);
    });
    view.read_with(window, |workspace, cx| {
        assert_eq!(workspace.spec.as_ref().unwrap().title, "After");
        let _ = cx;
        assert_eq!(workspace.spec.as_ref().unwrap().endpoints().count(), 1);
    });
}

/// Minimal HTTP server answering every request with `body` as JSON.
fn serve(status: &'static str, body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buf = [0u8; 8192];
            let _ = stream.read(&mut buf);
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    format!("http://{addr}")
}

#[gpui_kit::test]
fn saved_request_opens_only_one_tab(cx: &mut TestAppContext) {
    let mut project = project();
    project.collections[0]
        .requests
        .push(rivet::core::RequestSpec::new(
            "Delete order",
            Method::Delete,
            "{{baseUrl}}/orders/{id}",
        ));
    let (view, window) = open_with(cx, project);
    let before = view.read_with(window, |v, _| v.tabs.len());

    window.update(|w, cx| w.click(("saved", 0usize), cx));
    view.read_with(window, |v, _| {
        assert_eq!(v.tabs.len(), before + 1);
        let r = v.active().unwrap();
        assert_eq!(r.method, Method::Delete);
        assert_eq!(r.url, "{{baseUrl}}/orders/{id}");
    });

    window.update(|w, cx| w.click(("saved", 0usize), cx));
    assert_eq!(view.read_with(window, |v, _| v.tabs.len()), before + 1);
}

#[gpui_kit::test]
fn tabs_and_shortcuts(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| w.press("cmd-t", cx));
    view.read_with(window, |v, _| {
        assert_eq!(v.tabs.len(), 2);
        assert_eq!(v.active_tab, Some(1));
    });
    window.update(|w, cx| w.press("ctrl-tab", cx));
    assert_eq!(view.read_with(window, |v, _| v.active_tab), Some(0));
    window.update(|w, cx| w.press("cmd-w", cx));
    assert_eq!(view.read_with(window, |v, _| v.tabs.len()), 1);
    window.update(|w, cx| w.press("cmd-w", cx));
    view.read_with(window, |v, _| {
        assert!(v.tabs.is_empty());
        assert_eq!(v.active_tab, None);
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("send").is_none());
        assert!(w.try_find("empty-new-tab").is_some());
        w.press("cmd-enter", cx); // no tab: must not panic
    });
}

#[gpui_kit::test]
fn url_editing_is_kept_per_tab(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| {
        w.click("url", cx);
        w.press("cmd-a", cx);
        w.input("https://example.com/{{x}}", cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |v, cx| {
        assert_eq!(v.tabs[0].request.url, "https://example.com/{{x}}");
        assert_eq!(v.url_input.read(cx).tokens().len(), 1);
    });
    window.update(|w, cx| w.press("cmd-t", cx));
    window.update(|w, cx| w.click(("tab", 0usize), cx));
    view.read_with(window, |v, cx| {
        assert_eq!(
            v.url_input.read(cx).value().as_ref(),
            "https://example.com/{{x}}"
        );
    });
}

#[gpui_kit::test]
fn unnamed_request_uses_last_url_segment_as_name(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| {
        w.press("cmd-t", cx);
        w.click("url", cx);
        w.input("https://example.com/v1/widgets?active=1", cx);
    });
    view.read_with(window, |v, _| {
        assert_eq!(v.active().unwrap().name, "widgets");
    });
}

#[gpui_kit::test]
fn curl_paste_imports_request(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| {
        w.click("url", cx);
        w.press("cmd-a", cx);
        w.input(
            "curl -X PUT https://api.test/items/1 -H 'X-Trace: 7' -d '{\"a\":1}'",
            cx,
        );
    });
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |v, _| {
        let r = v.active().unwrap();
        assert_eq!(r.method, Method::Put);
        assert_eq!(r.url, "https://api.test/items/1");
        assert!(
            r.headers
                .iter()
                .any(|h| h.key == "X-Trace" && h.value == "7")
        );
    });
}

#[gpui_kit::test]
fn method_dropdown_selects(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| {
        w.click("method-select", cx);
        w.render_frame(cx);
        w.click(("method", Method::Patch as usize), cx);
    });
    view.read_with(window, |v, _| {
        assert_eq!(v.active().unwrap().method, Method::Patch);
        assert!(!v.method_menu_open);
    });
}

#[gpui_kit::test]
fn env_switch_cycles(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| {
        w.click("env-switch", cx);
        w.render_frame(cx);
        w.click(("env", 1usize), cx);
    });
    view.read_with(window, |v, _| {
        assert_eq!(v.env().unwrap().name, "Prod");
        assert!(!v.env_menu_open);
    });
    window.update(|w, cx| w.press("cmd-e", cx));
    assert_eq!(view.read_with(window, |v, _| v.project.active_env), 2);
}

#[gpui_kit::test]
fn sidebar_filter_and_rails(cx: &mut TestAppContext) {
    let mut project = project();
    project.collections[0].requests = vec![
        rivet::core::RequestSpec::new("List orders", Method::Get, "{{baseUrl}}/orders"),
        rivet::core::RequestSpec::new("Login", Method::Post, "{{baseUrl}}/auth/login"),
    ];
    let (view, window) = open_with(cx, project);
    window.update(|w, cx| {
        w.click("sidebar-search", cx);
        w.input("login", cx);
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find(("saved", 0usize)).is_none(), "orders hidden");
        assert!(w.try_find(("saved", 1usize)).is_some(), "auth/Login shown");
    });

    window.update(|w, cx| w.press("cmd-2", cx));
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("env-add").is_some(), "env panel shown");
    });
    window.update(|w, cx| w.press("cmd-3", cx));
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(
            w.try_find("history-filter").is_some(),
            "history panel shown"
        );
    });
    window.update(|w, cx| w.press("cmd-1", cx));
    assert!(view.read_with(window, |v, _| v.rail == RailMode::Requests));
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("api-editor").is_none());
    });
}

#[gpui_kit::test]
fn mode_rail_keeps_one_layout_when_switching_sections(cx: &mut TestAppContext) {
    let (_, window) = open(cx);
    window.update(|w, cx| {
        let modes = [
            RailMode::Requests,
            RailMode::Environments,
            RailMode::History,
        ];
        for mode in modes.into_iter().chain([RailMode::Requests]) {
            w.click(("rail", mode as usize), cx);
            w.render_frame(cx);
            assert_eq!(w.find("mode-rail").bounds().size.width, gpui_kit::px(48.));
            for item in modes {
                assert!(w.try_find(("rail", item as usize)).is_some());
            }
        }
    });
}

#[gpui_kit::test]
fn reload_refreshes_spec_without_editor(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("api.yaml");
    std::fs::write(
        &path,
        "openapi: 3.0.3\ninfo: {title: Before, version: 1}\npaths:\n  /before:\n    get:\n      responses:\n        '200': {description: OK}\n",
    )
    .unwrap();
    let mut project = project();
    project.spec_path = Some(path.clone());
    let (view, window) = open_with(cx, project);
    assert_eq!(
        view.read_with(window, |v, _| v.spec.as_ref().unwrap().endpoints().count()),
        1
    );

    std::fs::write(
        &path,
        "openapi: 3.0.3\ninfo: {title: After, version: 2}\npaths:\n  /after:\n    get:\n      responses:\n        '200': {description: OK}\n  /also-after:\n    post:\n      responses:\n        '201': {description: Created}\n",
    )
    .unwrap();
    window.update(|w, cx| {
        w.dispatch_action(Box::new(app::ReloadSpec), cx);
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("api-editor").is_none());
        assert!(w.try_find(("endpoint", 0usize)).is_none());
    });
    view.read_with(window, |v, cx| {
        assert_eq!(v.spec.as_ref().unwrap().title, "After");
        let _ = cx;
        assert_eq!(v.spec.as_ref().unwrap().endpoints().count(), 2);
    });
}

#[gpui_kit::test]
fn save_request_to_collection(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| {
        w.press("cmd-s", cx);
        w.render_frame(cx);
        assert!(w.try_find("collection-dialog").is_some());
        w.click("dialog-confirm", cx);
    });
    assert_eq!(
        view.read_with(window, |v, _| v.project.collections[0].requests.len()),
        1
    );
    // Saving again updates in place.
    window.update(|w, cx| w.press("cmd-s", cx));
    assert_eq!(
        view.read_with(window, |v, _| v.project.collections[0].requests.len()),
        1
    );
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find(("saved", 0usize)).is_some());
    });
}

#[gpui_kit::test]
fn save_dialog_chooses_collection_folder_and_name(cx: &mut TestAppContext) {
    let mut p = project();
    let mut second = Collection::new("Команда");
    second.folders.push("Inbox".into());
    let second_id = second.id.clone();
    p.collections.push(second);
    let (view, window) = open_with(cx, p);
    window.update(|w, cx| {
        w.press("cmd-s", cx);
        w.render_frame(cx);
        w.click(("dialog-collection", 1usize), cx);
        w.render_frame(cx);
        w.click(("dialog-folder", 0usize), cx);
        w.click("collection-dialog-name", cx);
        w.press("cmd-a", cx);
        w.input("Find orders", cx);
        w.click("dialog-confirm", cx);
        w.render_frame(cx);
    });
    view.read_with(window, |v, _| {
        assert!(v.project.collections[0].requests.is_empty());
        let saved = &v.project.collections[1].requests[0];
        assert_eq!(saved.name, "Find orders");
        assert_eq!(saved.folder.as_deref(), Some("Inbox"));
        assert_eq!(saved.collection_id.as_deref(), Some(second_id.as_str()));
    });
}

#[gpui_kit::test]
fn saved_request_drags_between_collections_and_folders(cx: &mut TestAppContext) {
    let mut p = project();
    let mut first = collection_with_request();
    let request_id = first.requests[0].id.clone();
    let first_id = first.id.clone();
    first.folders.push("Archive".into());
    p.collections = vec![first, Collection::new("Team")];
    let second_id = p.collections[1].id.clone();
    let (view, window) = open_with(cx, p);
    window.update(|w, cx| {
        w.render_frame(cx);
        w.drag_to(("saved-drag", 0usize), ("collection-drop", 1usize), cx);
        w.render_frame(cx);
    });
    view.read_with(window, |v, _| {
        assert!(v.project.collections[0].requests.is_empty());
        assert_eq!(v.project.collections[1].requests[0].id, request_id);
        assert_eq!(
            v.project.collections[1].requests[0]
                .collection_id
                .as_deref(),
            Some(second_id.as_str())
        );
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        w.drag_to(("saved-drag", 1000usize), ("folder-drop", 0usize), cx);
        w.render_frame(cx);
    });
    view.read_with(window, |v, _| {
        assert!(v.project.collections[1].requests.is_empty());
        assert_eq!(
            v.project.collections[0].requests[0].folder.as_deref(),
            Some("Archive")
        );
        assert_eq!(
            v.project.collections[0].requests[0]
                .collection_id
                .as_deref(),
            Some(first_id.as_str())
        );
    });
}

fn collection_with_request() -> Collection {
    let mut col = Collection::new("Personal");
    let mut req =
        rivet::core::RequestSpec::new("Orders", Method::Get, "https://example.com/orders");
    req.collection_id = Some(col.id.clone());
    col.requests.push(req);
    col
}

#[gpui_kit::test]
fn collection_menu_creates_empty_folder_and_request_menu_renames(cx: &mut TestAppContext) {
    let mut p = project();
    p.collections = vec![collection_with_request()];
    let (view, window) = open_with(cx, p);
    window.update(|w, cx| {
        w.click("sidebar-menu-trigger-collection-0", cx);
        w.render_frame(cx);
        assert!(
            w.try_find("sidebar-action-collection-0-new-folder")
                .is_some()
        );
        w.click("sidebar-action-collection-0-new-folder", cx);
        w.render_frame(cx);
        assert!(w.try_find("collection-dialog").is_some());
        w.click("collection-dialog-name", cx);
        w.input("Archive", cx);
        w.click("dialog-confirm", cx);
        w.render_frame(cx);
        assert_eq!(
            view.read(cx).project.collections[0].folders,
            vec!["Archive"]
        );
        assert!(w.try_find(("collection-folder", 0usize)).is_some());
        w.click("sidebar-menu-trigger-request-0-0", cx);
        w.render_frame(cx);
        w.click("sidebar-action-request-0-0-rename", cx);
        w.render_frame(cx);
        w.click("collection-dialog-name", cx);
        w.press("cmd-a", cx);
        w.input("New name", cx);
        w.click("dialog-confirm", cx);
    });
    view.read_with(window, |v, _| {
        assert_eq!(v.project.collections[0].folders, vec!["Archive"]);
        assert_eq!(v.project.collections[0].requests[0].name, "New name");
    });
}

#[gpui_kit::test]
fn nested_collection_folders_collapse_as_a_tree(cx: &mut TestAppContext) {
    let mut p = project();
    let mut col = collection_with_request();
    col.folders = vec!["Team / QA".into()];
    col.requests[0].folder = Some("Team / QA".into());
    p.collections = vec![col];
    let (_, window) = open_with(cx, p);
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find(("collection-folder", 0usize)).is_some());
        assert!(w.try_find(("collection-folder", 1usize)).is_some());
        assert!(w.try_find(("saved", 0usize)).is_some());
        w.click(("collection-folder", 0usize), cx);
        w.render_frame(cx);
        assert!(w.try_find(("collection-folder", 1usize)).is_none());
        assert!(w.try_find(("saved", 0usize)).is_none());
    });
}

#[gpui_kit::test]
fn production_destructive_needs_confirmation(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| {
        w.click("method-select", cx);
        w.render_frame(cx);
        w.click(("method", Method::Post as usize), cx);
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        w.click("env-switch", cx);
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        w.click(("env", 1usize), cx); // Prod
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        w.click("send", cx);
    });
    view.read_with(window, |v, _| {
        assert!(v.confirm_open);
        assert!(matches!(
            v.tabs[v.active_tab.unwrap()].response,
            ResponseState::Empty
        ));
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        w.click("cancel", cx);
    });
    assert!(!view.read_with(window, |v, _| v.confirm_open));
}

#[gpui_kit::test]
fn palette_opens_and_runs_command(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| w.press("cmd-k", cx));
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("command-palette").is_some());
        w.input("тему", cx);
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        w.press("enter", cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    assert!(view.read_with(window, |v, _| v.palette.is_none()));
    assert_eq!(
        window.update(|_, cx| cx.global::<Palette>().mode),
        Mode::Light
    );
}

#[gpui_kit::test]
fn send_real_request_records_history(cx: &mut TestAppContext) {
    let base = serve("200 OK", r#"{"ok":true}"#);
    let mut p = project();
    p.environments[0].vars.retain(|v| v.key != "baseUrl");
    p.environments[0].vars.push(KeyValue::new("baseUrl", base));
    let (view, window) = open_with(cx, p);

    window.update(|w, cx| w.press("cmd-enter", cx));
    view.read_with(window, |v, _| {
        assert!(matches!(v.tabs[0].response, ResponseState::Loading { .. }));
    });
    // Real network IO on a background thread: let it finish.
    for _ in 0..200 {
        std::thread::sleep(Duration::from_millis(20));
        cx.run_until_parked();
        if view.read_with(cx, |v, _| !v.tabs[0].is_sending()) {
            break;
        }
    }
    view.read_with(cx, |v, _| {
        match &v.tabs[0].response {
            ResponseState::Done { response, .. } => assert_eq!(response.status, 200),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(v.project.history.len(), 1);
        assert_eq!(v.project.history[0].status, Some(200));
    });
}

#[gpui_kit::test]
fn body_variables_are_highlighted_and_substituted(cx: &mut TestAppContext) {
    use std::sync::{Arc, Mutex};

    // Echo-ish server that remembers the last raw request.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(String::new()));
    {
        let seen = seen.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut buf = vec![0u8; 16384];
                let mut raw = Vec::new();
                // Read until headers + Content-Length body are in.
                loop {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    raw.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&raw).to_string();
                    if let Some(split) = text.find("\r\n\r\n") {
                        let len = text
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if raw.len() >= split + 4 + len || n == 0 {
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                *seen.lock().unwrap() = String::from_utf8_lossy(&raw).to_string();
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                );
            }
        });
    }

    let mut p = project();
    p.environments[0].vars.retain(|v| v.key != "baseUrl");
    p.environments[0]
        .vars
        .push(KeyValue::new("baseUrl", base.clone()));
    p.open_tabs = vec![{
        let mut r = rivet::core::RequestSpec::new("Echo", Method::Post, "{{baseUrl}}/echo");
        r.body = rivet::core::Body::json("{}");
        r
    }];
    let (view, window) = open_with(cx, p);

    let body = "{\n  \"rweqr\": \"qwerrqwe\",\n  \"test2\": \"{{baseUrl}}\"\n}";
    window.update(|w, cx| {
        let editor = view.read(cx).editor.clone();
        editor.update(cx, |e, cx| e.set_body_text(body.to_string(), w, cx));
    });
    window.update(|w, cx| w.render_frame(cx));

    // Highlighted in the editor: exactly the `{{baseUrl}}` span.
    view.read_with(window, |v, cx| {
        let start = body.find("{{baseUrl}}").unwrap();
        assert_eq!(
            v.editor.read(cx).body_var_ranges(cx),
            vec![start..start + "{{baseUrl}}".len()]
        );
        assert_eq!(
            v.tabs[0].request.body.text, body,
            "body synced into the tab"
        );
    });

    window.update(|w, cx| w.press("cmd-enter", cx));
    for _ in 0..200 {
        std::thread::sleep(Duration::from_millis(20));
        cx.run_until_parked();
        if view.read_with(cx, |v, _| !v.tabs[0].is_sending()) {
            break;
        }
    }
    let raw = seen.lock().unwrap().clone();
    assert!(
        raw.contains(&format!("\"test2\": \"{base}\"")),
        "substituted body sent: {raw}"
    );
    assert!(!raw.contains("{{baseUrl}}"));
}

#[gpui_kit::test]
fn gpui_clicks_take_keyboard_back_from_browser(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    let releases = |window: &mut VisualTestContext| {
        view.read_with(window, |v, cx| {
            v.response.read(cx).browser_keyboard_releases()
        })
    };
    let before = releases(window);
    // Clicking the URL field (or anything GPUI draws) must hand the keyboard
    // back, otherwise WebKit keeps swallowing key presses.
    window.update(|w, cx| w.click("url", cx));
    assert!(releases(window) > before);
    window.update(|w, cx| {
        w.input("/typed", cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    assert!(view.read_with(window, |v, _| v.tabs[0].request.url.ends_with("/typed")));
}

/// Server that answers `{"searchId":"abc123"}` and records request lines.
fn serve_recording() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::sync::{Arc, Mutex};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]).lines().next().unwrap_or("").to_string();
            log.lock().unwrap().push(head);
            let body = r#"{"searchId":"abc123"}"#;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (base, seen)
}

fn wait_sent(view: &Entity<Workspace>, window: &mut VisualTestContext, tab: usize) {
    for _ in 0..300 {
        std::thread::sleep(Duration::from_millis(20));
        window.run_until_parked();
        if view.read_with(window, |v, _| !v.tabs[tab].is_sending()) {
            return;
        }
    }
    panic!("request did not finish");
}

#[gpui_kit::test]
fn post_response_script_chains_requests(cx: &mut TestAppContext) {
    let (base, seen) = serve_recording();
    let mut p = project();
    p.environments[0].vars.retain(|v| v.key != "baseUrl");
    p.environments[0].vars.push(KeyValue::new("baseUrl", base));
    let mut search = rivet::core::RequestSpec::new("Search", Method::Post, "{{baseUrl}}/search");
    search.scripts.post_response = "let response = JSON.parse(responseBody);\npm.environment.set(\"searchId\", response.searchId);\npm.test(\"has id\", () => pm.expect(response.searchId).to.equal(\"abc123\"));".into();
    let details = rivet::core::RequestSpec::new("Details", Method::Get, "{{baseUrl}}/s/{{searchId}}");
    p.open_tabs = vec![search, details];
    p.active_tab = Some(0);
    let (view, window) = open_with(cx, p);

    window.update(|w, cx| w.press("cmd-enter", cx));
    wait_sent(&view, window, 0);
    view.read_with(window, |v, _| {
        assert_eq!(v.project.environments[0].var("searchId"), Some("abc123"), "script persisted the variable");
        let report = v.tabs[0].script_report.as_ref().expect("script report");
        assert!(report.tests().all(|t| t.passed), "{report:?}");
    });

    window.update(|w, cx| w.click(("tab", 1usize), cx));
    window.update(|w, cx| w.press("cmd-enter", cx));
    wait_sent(&view, window, 1);
    let lines = seen.lock().unwrap().clone();
    assert!(lines.iter().any(|l| l.starts_with("GET /s/abc123 ")), "{lines:?}");
}

#[gpui_kit::test]
fn failing_pre_request_script_blocks_send(cx: &mut TestAppContext) {
    let (base, seen) = serve_recording();
    let mut p = project();
    p.environments[0].vars.retain(|v| v.key != "baseUrl");
    p.environments[0].vars.push(KeyValue::new("baseUrl", base));
    let mut r = rivet::core::RequestSpec::new("Broken", Method::Get, "{{baseUrl}}/x");
    r.scripts.pre_request = "const a = 1;\nthrow new Error(\"boom\");".into();
    p.open_tabs = vec![r];
    let (view, window) = open_with(cx, p);
    window.update(|w, cx| w.press("cmd-enter", cx));
    wait_sent(&view, window, 0);
    view.read_with(window, |v, _| match &v.tabs[0].response {
        ResponseState::Failed(rivet::core::SendError::Script { message, line }) => {
            assert!(message.contains("boom"));
            assert_eq!(*line, Some(2));
        }
        other => panic!("unexpected {other:?}"),
    });
    assert!(seen.lock().unwrap().is_empty(), "nothing was sent");
}

#[gpui_kit::test]
fn collection_variables_set_persists_in_collection(cx: &mut TestAppContext) {
    let (base, _) = serve_recording();
    let mut p = project();
    p.environments[0].vars.retain(|v| v.key != "baseUrl");
    p.environments[0].vars.push(KeyValue::new("baseUrl", base));
    let collection_id = p.collections[0].id.clone();
    let mut r = rivet::core::RequestSpec::new("Search", Method::Get, "{{baseUrl}}/search");
    r.collection_id = Some(collection_id.clone());
    r.scripts.post_response =
        "const json = pm.response.json();\npm.collectionVariables.set(\"tokentmp\", json.searchId);".into();
    p.open_tabs = vec![r];
    let (view, window) = open_with(cx, p);
    window.update(|w, cx| w.press("cmd-enter", cx));
    wait_sent(&view, window, 0);
    view.read_with(window, |v, _| {
        let col = v.project.collections.iter().find(|c| c.id == collection_id).unwrap();
        assert!(col.vars.iter().any(|kv| kv.key == "tokentmp" && kv.value == "abc123"), "{:?}", col.vars);
        assert_eq!(v.project.environments[0].var("tokentmp"), None, "environment untouched");
    });
}

#[gpui_kit::test]
fn sidebar_width_is_draggable_clamped_and_persisted(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    let start = view.read_with(window, |v, _| v.sidebar_width);
    assert_eq!(start, 260.);

    // Drag the splitter 120 px to the right.
    window.update(|w, cx| {
        let b = w.find("sidebar-splitter").bounds();
        let from = b.center();
        let to = gpui_kit::point(from.x + gpui_kit::px(120.), from.y);
        w.drag(from, to, cx);
    });
    let dragged = view.read_with(window, |v, _| v.sidebar_width);
    assert!((dragged - 380.).abs() < 2., "width {dragged}");
    assert_eq!(view.read_with(window, |v, _| v.project.sidebar_width), Some(dragged.round() as u32));

    // Way past the limits clamps to min / max.
    view.update(window, |v, cx| v.set_sidebar_width(5000., cx));
    assert_eq!(view.read_with(window, |v, _| v.sidebar_width), 520.);
    view.update(window, |v, cx| v.set_sidebar_width(10., cx));
    assert_eq!(view.read_with(window, |v, _| v.sidebar_width), 200.);

    // Double click resets to the default and clears the stored width.
    window.update(|w, cx| {
        w.render_frame(cx);
        w.double_click("sidebar-splitter", cx);
    });
    view.read_with(window, |v, _| {
        assert_eq!(v.sidebar_width, 260.);
        assert_eq!(v.project.sidebar_width, None);
    });
}

#[gpui_kit::test]
fn stored_sidebar_width_is_restored(cx: &mut TestAppContext) {
    let mut p = project();
    p.sidebar_width = Some(410);
    let (view, window) = open_with(cx, p);
    assert_eq!(view.read_with(window, |v, _| v.sidebar_width), 410.);
    window.update(|w, cx| {
        let rail = w.find("mode-rail").bounds();
        let splitter = w.find("sidebar-splitter").bounds();
        let sidebar = splitter.origin.x.as_f32() - (rail.origin.x + rail.size.width).as_f32();
        assert!((sidebar - 410.).abs() < 1.5, "sidebar {sidebar}");
        let _ = cx;
    });
}

#[gpui_kit::test]
fn unsaved_dot_and_close_prompt(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    // A request saved in a collection, opened in a tab, is clean.
    window.update(|w, cx| w.press("cmd-s", cx));
    let saved = view.read_with(window, |v, _| v.project.collections[0].requests.len());
    if saved == 0 {
        // First save goes through the "save to collection" dialog in this build;
        // save programmatically instead.
        view.update(window, |v, cx| {
            let r = v.active().unwrap().clone();
            v.project.collections[0].requests.push(r);
            cx.notify();
        });
    }
    let dirty_dot = |w: &mut gpui_kit::Window| {
        w.try_find(gpui_kit::ElementId::NamedChild(
            std::sync::Arc::new(gpui_kit::ElementId::NamedChild(
                std::sync::Arc::new(("tab", 0usize).into()),
                "close".into(),
            )),
            "dirty".into(),
        ))
        .is_some()
    };
    window.update(|w, cx| w.render_frame(cx));
    assert!(!view.read_with(window, |v, _| v.tab_unsaved(0)));
    window.update(|w, _| assert!(!dirty_dot(w)));

    // Editing the URL marks the tab unsaved and shows the dot.
    window.update(|w, cx| {
        w.click("url", cx);
        w.press("cmd-a", cx);
        w.input("https://example.com/changed", cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    assert!(view.read_with(window, |v, _| v.tab_unsaved(0)));
    window.update(|w, _| assert!(dirty_dot(w)));

    // Closing asks; Cancel keeps the tab, "Не сохранять" closes it.
    let tabs = view.read_with(window, |v, _| v.tabs.len());
    window.update(|w, cx| w.press("cmd-w", cx));
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("close-confirm").is_some());
        w.click("close-confirm-cancel", cx);
    });
    assert_eq!(view.read_with(window, |v, _| v.tabs.len()), tabs);
    window.update(|w, cx| w.press("cmd-w", cx));
    window.update(|w, cx| {
        w.render_frame(cx);
        w.click("close-confirm-discard", cx);
    });
    assert_eq!(view.read_with(window, |v, _| v.tabs.len()), tabs - 1);
}

#[gpui_kit::test]
fn env_menu_creates_environment_even_when_there_are_none(cx: &mut TestAppContext) {
    let (view, window) = open_with(cx, store::seed_project());
    window.update(|w, cx| {
        w.click("env-switch", cx);
        w.render_frame(cx);
        assert!(w.try_find("env-create").is_some());
        w.click("env-create", cx);
        w.render_frame(cx);
    });
    view.read_with(window, |v, _| {
        assert!(!v.env_menu_open);
        assert!(v.rail == RailMode::Environments);
        assert_eq!(v.project.environments.len(), 1);
        assert_eq!(v.project.active_env, 0);
        assert!(v.env().is_some());
    });

    // With environments the row is still there, below the list.
    let (view, window) = open(cx);
    window.update(|w, cx| {
        w.click("env-switch", cx);
        w.render_frame(cx);
        let last = w.find(("env", 2usize)).bounds();
        let create = w.find("env-create").bounds();
        assert!(create.origin.y > last.origin.y);
        w.click("env-create", cx);
        w.render_frame(cx);
    });
    view.read_with(window, |v, _| {
        assert_eq!(v.project.environments.len(), 4);
        assert_eq!(v.project.active_env, 0, "creating does not switch the active env");
        assert!(v.rail == RailMode::Environments);
    });
}

fn open_tabs(cx: &mut TestAppContext, count: usize) -> (Entity<Workspace>, &mut VisualTestContext) {
    let mut project = project();
    project.open_tabs = (0..count)
        .map(|i| rivet::core::RequestSpec::new(format!("Request {i}"), Method::Get, format!("http://h/{i}")))
        .collect();
    project.active_tab = Some(0);
    open_with(cx, project)
}

#[gpui_kit::test]
fn dragging_a_tab_reorders_tabs_and_keeps_the_active_one(cx: &mut TestAppContext) {
    let (view, window) = open_tabs(cx, 3);
    let names = |view: &Entity<Workspace>, window: &mut VisualTestContext| {
        view.read_with(window, |v, _| {
            v.tabs.iter().map(|t| t.request.name.clone()).collect::<Vec<_>>()
        })
    };
    window.update(|w, cx| w.drag_to(("tab", 0usize), ("tab", 2usize), cx));
    assert_eq!(names(&view, window), ["Request 1", "Request 2", "Request 0"]);
    view.read_with(window, |v, _| {
        assert_eq!(v.active_tab, Some(2));
        assert_eq!(v.active().unwrap().name, "Request 0");
    });
    window.update(|w, cx| w.drag_to(("tab", 1usize), ("tab", 0usize), cx));
    assert_eq!(names(&view, window), ["Request 2", "Request 1", "Request 0"]);
    view.read_with(window, |v, _| assert_eq!(v.active_tab, Some(2)));
    // A click (no movement) still just activates.
    window.update(|w, cx| w.click(("tab", 0usize), cx));
    assert_eq!(names(&view, window), ["Request 2", "Request 1", "Request 0"]);
    view.read_with(window, |v, _| assert_eq!(v.active_tab, Some(0)));
}

#[gpui_kit::test]
fn new_tab_button_follows_last_tab_then_sticks_right(cx: &mut TestAppContext) {
    let (_, window) = open_tabs(cx, 2);
    window.update(|w, cx| {
        w.render_frame(cx);
        let last = w.find(("tab", 1usize)).bounds();
        let plus = w.find("new-tab-area").bounds();
        let gap = plus.origin.x.as_f32() - (last.origin.x + last.size.width).as_f32();
        assert!(gap.abs() <= 2., "`+` right after the last tab, gap {gap}");
        assert!(plus.origin.x.as_f32() + plus.size.width.as_f32() < w.viewport_size().width.as_f32() - 100.);
    });

    let (_, window) = open_tabs(cx, 40);
    window.update(|w, cx| {
        w.render_frame(cx);
        let plus = w.find("new-tab-area").bounds();
        let right = plus.origin.x.as_f32() + plus.size.width.as_f32();
        let bar = w.find("tabbar").bounds();
        // Visible, at the right edge of the strip, and the tabs end before it.
        assert!(right <= w.viewport_size().width.as_f32() + 0.5, "`+` off screen at {right}");
        assert!((bar.origin.x + bar.size.width).as_f32() <= plus.origin.x.as_f32() + 0.5);
        assert!(w.viewport_size().width.as_f32() - right < 2., "`+` sticks to the right: {right}");
    });
}

#[gpui_kit::test]
fn undefined_variables_are_sent_empty_with_a_warning(cx: &mut TestAppContext) {
    let (base, seen) = serve_recording();
    let mut p = project();
    p.environments[0].vars.retain(|v| v.key != "baseUrl");
    p.environments[0].vars.push(KeyValue::new("baseUrl", base));
    let mut request = rivet::core::RequestSpec::new("Q", Method::Get, "{{baseUrl}}/q");
    request.params = vec![
        KeyValue::new("{{test111}}", "12321"),
        KeyValue::new("v", "{{v}}"),
        KeyValue::new("customer", "{{customerId}}"),
    ];
    p.open_tabs = vec![request];
    p.active_tab = Some(0);
    let (view, window) = open_with(cx, p);

    window.update(|w, cx| {
        w.render_frame(cx);
        // Warned before sending.
        assert!(w.try_find("missing-vars").is_some());
        w.press("cmd-enter", cx);
    });
    wait_sent(&view, window, 0);
    let line = seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(line, "GET /q?v=&customer=c_51b2e7a0 HTTP/1.1");
    assert!(!line.contains("%7B"));
    view.read_with(window, |v, _| {
        assert!(matches!(v.tabs[0].response, ResponseState::Done { .. }));
        let notice = v.notice.as_deref().unwrap_or_default();
        assert!(notice.contains("{{test111}}") && notice.contains("{{v}}"), "{notice}");
    });
}

#[gpui_kit::test]
fn pasting_multiline_curl_into_url_field_imports_it(cx: &mut TestAppContext) {
    let (view, window) = open_with(cx, store::seed_project());
    // As copied from GitHub docs: `\` line continuations, a `$ ` prompt.
    let cmd = "$ curl -L \\\n  -H \"Accept: application/vnd.github+json\" \\\n  -H \"Authorization: Bearer <YOUR-TOKEN>\" \\\n  -H \"X-GitHub-Api-Version: 2022-11-28\" \\\n  https://api.github.com/repos/OWNER/REPO/issues";
    window.update(|w, cx| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(cmd.to_string()));
        w.click("url", cx);
        w.press("cmd-v", cx);
        w.render_frame(cx);
    });
    window.run_until_parked();
    view.read_with(window, |v, _| {
        let r = v.active().unwrap();
        assert_eq!(r.url, "https://api.github.com/repos/OWNER/REPO/issues");
        assert_eq!(
            r.headers,
            vec![
                KeyValue::new("Accept", "application/vnd.github+json"),
                KeyValue::new("X-GitHub-Api-Version", "2022-11-28"),
            ]
        );
        assert_eq!(v.notice.as_deref(), Some("cURL импортирован"));
    });
}

#[gpui_kit::test]
fn invalid_header_is_not_sent_and_explains_why(cx: &mut TestAppContext) {
    let (base, seen) = serve_recording();
    let mut p = project();
    let mut request = rivet::core::RequestSpec::new("Q", Method::Get, format!("{base}/q"));
    request.headers = vec![KeyValue::new("Cookie", "a=1\nb=2")];
    p.open_tabs = vec![request];
    p.active_tab = Some(0);
    let (view, window) = open_with(cx, p);
    window.update(|w, cx| w.press("cmd-enter", cx));
    wait_sent(&view, window, 0);
    view.read_with(window, |v, _| match &v.tabs[0].response {
        ResponseState::Failed(rivet::core::SendError::InvalidHeader { name, message }) => {
            assert_eq!(name, "Cookie");
            assert_eq!(message, "В заголовке не может быть перевода строки");
        }
        other => panic!("{other:?}"),
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("response-error-pill").is_some());
    });
    assert!(seen.lock().unwrap().is_empty(), "nothing reached the server");
}

#[gpui_kit::test]
fn collections_export_in_three_formats(cx: &mut TestAppContext) {
    use rivet::core::export::ExportFormat;
    let mut p = project();
    let mut list = rivet::core::RequestSpec::new("List", Method::Get, "{{baseUrl}}/orders");
    list.collection_id = Some(p.collections[0].id.clone());
    p.collections[0].requests.push(list);
    let id = p.collections[0].id.clone();
    let (view, window) = open_with(cx, p);
    // Offered in the collection's "···" menu.
    window.update(|w, cx| {
        w.click("sidebar-menu-trigger-collection-0", cx);
        w.render_frame(cx);
        for action in [
            "sidebar-action-collection-0-export-rivet",
            "sidebar-action-collection-0-export-postman",
            "sidebar-action-collection-0-export-openapi",
        ] {
            assert!(w.try_find(action).is_some(), "{action}");
        }
    });
    // And on the Collections page.
    view.update_in(window, |v, window, cx| v.set_rail(RailMode::Collections, window, cx));
    window.update(|w, cx| {
        w.render_frame(cx);
        for i in 0..3usize {
            assert!(w.try_find(("collection-export", i)).is_some());
        }
    });
    let dir = std::env::temp_dir().join(format!("rivet-ws-export-{}", rivet::core::new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    view.read_with(window, |v, _| {
        for format in ExportFormat::ALL {
            let path = dir.join(format.file_name("Мои запросы"));
            let warnings = v.export_collection_to(&id, format, &path).unwrap();
            assert!(warnings.is_empty(), "{warnings:?}");
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(text.contains("/orders"), "{format:?}: {text}");
        }
        assert!(v.export_collection_to("nope", ExportFormat::Rivet, &dir.join("x")).is_err());
    });
    // Re-importing the Rivet file adds a copy of the collection.
    view.update_in(window, |v, window, cx| {
        v.import_file(dir.join(ExportFormat::Rivet.file_name("Мои запросы")), window, cx)
    });
    view.read_with(window, |v, _| {
        assert_eq!(v.project.collections.len(), 2);
        assert_eq!(v.project.collections[1].name, "Мои запросы");
        assert_ne!(v.project.collections[1].id, id);
        assert_eq!(v.project.collections[1].requests[0].name, "List");
    });
    std::fs::remove_dir_all(&dir).ok();
}

#[gpui_kit::test]
fn save_state_shows_saved_modified_and_draft(cx: &mut TestAppContext) {
    use rivet::app::SaveState;
    let mut p = project();
    p.collections[0].folders = vec!["Orders".into()];
    let mut saved = rivet::core::RequestSpec::new("List orders", Method::Get, "{{baseUrl}}/orders");
    saved.collection_id = Some(p.collections[0].id.clone());
    saved.folder = Some("Orders".into());
    p.collections[0].requests.push(saved.clone());
    p.open_tabs = vec![saved];
    p.active_tab = Some(0);
    let (view, window) = open_with(cx, p);

    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("save-state-saved").is_some());
        assert!(w.within(("saved", 0usize)).try_find("modified").is_none());
    });
    view.read_with(window, |v, _| {
        assert_eq!(
            v.tab_save_state(0),
            Some(SaveState::Saved { collection: "Мои запросы".into(), folder: Some("Orders".into()) })
        );
    });

    // An edit in the tab: modified everywhere it shows.
    window.update(|w, cx| {
        w.click("url", cx);
        w.press("cmd-right", cx);
        w.input("/42", cx);
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("save-state-modified").is_some());
        assert!(w.within(("saved", 0usize)).try_find("modified").is_some());
        assert!(w.try_find(child(child(("tab", 0usize), "close"), "dirty")).is_some());
    });
    view.read_with(window, |v, _| {
        assert!(matches!(v.tab_save_state(0), Some(SaveState::Modified { .. })));
        assert_eq!(v.modified_request_ids(), vec![v.tabs[0].request.id.clone()]);
    });

    // ⌘S writes it to the collection.
    window.update(|w, cx| w.press("cmd-s", cx));
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("save-state-saved").is_some());
        assert!(w.within(("saved", 0usize)).try_find("modified").is_none());
    });
    view.read_with(window, |v, _| {
        assert_eq!(v.project.collections[0].requests[0].url, "{{baseUrl}}/orders/42");
        assert!(v.modified_request_ids().is_empty());
    });

    // A new tab is a draft outside collections; "Сохранить…" asks where.
    view.update_in(window, |v, window, cx| v.new_tab(window, cx));
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("save-state-draft").is_some());
        w.click("save-to-collection", cx);
        w.render_frame(cx);
        assert!(w.try_find("collection-dialog").is_some());
    });
    view.read_with(window, |v, _| assert_eq!(v.tab_save_state(1), Some(SaveState::Draft)));
}
