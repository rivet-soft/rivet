use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, point, px};
use rivet::app::request_editor::{
    RequestEditor, RequestEditorEvent, RequestEditorTab, ScriptStage,
};
use rivet::core::types::{Auth, BodyKind, KeyValue, RequestSpec, Scripts, SpecRef};
use rivet::model::Method;
use rivet::theme::Palette;

fn sample() -> RequestSpec {
    let mut spec = RequestSpec::new("Create order", Method::Post, "{{baseUrl}}/orders");
    spec.params = vec![
        KeyValue::new("limit", "10"),
        KeyValue::new("page", "1").enabled(false),
    ];
    spec.headers = vec![KeyValue::new("Accept", "application/json")];
    spec
}

fn open(
    cx: &mut TestAppContext,
    spec: RequestSpec,
) -> (
    Entity<RequestEditor>,
    &mut VisualTestContext,
    Rc<RefCell<Vec<RequestEditorEvent>>>,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        Palette::dark().apply(cx);
    });
    let (view, window) = cx.add_window_view(|window, cx| RequestEditor::new(spec, window, cx));
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    window.update(|w, cx| {
        cx.subscribe(&view, move |_, ev: &RequestEditorEvent, _| {
            sink.borrow_mut().push(ev.clone())
        })
        .detach();
        w.render_frame(cx);
    });
    (view, window, events)
}

#[gpui_kit::test]
fn switches_sub_tabs_and_counts(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx, sample());
    view.read_with(window, |v, _| {
        assert_eq!(v.active_tab(), RequestEditorTab::Params);
        // Disabled "page" isn't counted.
        assert_eq!(v.count(RequestEditorTab::Params), 1);
        assert_eq!(v.count(RequestEditorTab::Headers), 1);
    });
    window.update(|w, cx| {
        assert!(w.within("req-params").try_find(("key", 0usize)).is_some());
        w.click(("subtab", 1usize), cx);
        w.render_frame(cx);
        assert!(w.try_find("req-params").is_none());
        assert!(w.within("req-headers").try_find(("key", 0usize)).is_some());
        w.click(("subtab", 3usize), cx);
        w.render_frame(cx);
        assert!(w.try_find("auth-type").is_some());
    });
    assert_eq!(
        view.read_with(window, |v, _| v.active_tab()),
        RequestEditorTab::Auth
    );
}

#[gpui_kit::test]
fn request_redirect_settings_override_and_inherit(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx, sample());
    window.update(|w, cx| {
        w.click(("subtab", RequestEditorTab::Settings as usize), cx);
        w.render_frame(cx);
        assert!(w.try_find("request-redirect-settings").is_some());
        w.within("request-follow-redirects").click(2usize, cx);
        w.click("request-redirect-limit", cx);
        w.input("3", cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |editor, _| {
        assert_eq!(editor.spec().follow_redirects, Some(false));
        assert_eq!(editor.spec().max_redirects, Some(3));
    });
    window.update(|w, cx| {
        w.within("request-follow-redirects").click(0usize, cx);
        w.click("request-redirect-limit", cx);
        w.press("cmd-a", cx);
        w.press("backspace", cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |editor, _| {
        assert_eq!(editor.spec().follow_redirects, None);
        assert_eq!(editor.spec().max_redirects, None);
    });
}

#[gpui_kit::test]
fn body_stays_visible_and_vertical_divider_respects_minimum_heights(cx: &mut TestAppContext) {
    let (_, window, _) = open(cx, sample());
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.within("req-params").try_find(("key", 0usize)).is_some());
        assert!(w.try_find("body-mode").is_some());
        w.click(("subtab", RequestEditorTab::Headers as usize), cx);
        w.render_frame(cx);
        assert!(w.within("req-headers").try_find(("key", 0usize)).is_some());
        assert!(w.try_find("body-mode").is_some());

        let initial = w.find("request-details-pane").bounds().size.height.as_f32();
        let from = w.find("request-body-divider").bounds().center();
        w.drag(from, from + point(px(0.), px(80.)), cx);
        w.render_frame(cx);
        let expanded = w.find("request-details-pane").bounds().size.height.as_f32();
        assert!(
            expanded > initial + 70.,
            "details pane should follow the divider"
        );

        let from = w.find("request-body-divider").bounds().center();
        w.drag(from, from - point(px(0.), px(1000.)), cx);
        w.render_frame(cx);
        let details = w.find("request-details-pane").bounds().size.height.as_f32();
        assert!(
            (details - 140.).abs() < 2.,
            "details pane must stop at 140 px"
        );

        let from = w.find("request-body-divider").bounds().center();
        w.drag(from, from + point(px(0.), px(1200.)), cx);
        w.render_frame(cx);
        let body = w.find("request-body-pane").bounds().size.height.as_f32();
        assert!((body - 170.).abs() < 2., "body pane must stop at 170 px");
    });
}

#[gpui_kit::test]
fn typing_into_draft_row_adds_header(cx: &mut TestAppContext) {
    let (view, window, events) = open(cx, sample());
    window.update(|w, cx| {
        w.click(("subtab", 1usize), cx);
        w.render_frame(cx);
        // Row 1 is the empty draft row.
        w.within("req-headers").click(("key", 1usize), cx);
        w.input("X-Trace", cx);
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        w.within("req-headers").click(("value", 1usize), cx);
        w.input("Bearer {{token}}", cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |v, _| {
        let h = &v.spec().headers;
        assert_eq!(h.len(), 2);
        assert_eq!(h[1].key, "X-Trace");
        assert_eq!(h[1].value, "Bearer {{token}}");
        assert!(h[1].enabled);
        assert_eq!(v.count(RequestEditorTab::Headers), 2);
    });
    // A fresh draft row exists after the promoted one.
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.within("req-headers").try_find(("key", 2usize)).is_some());
        assert!(w.within("req-headers").try_find(("key", 3usize)).is_none());
    });
    assert!(events.borrow().contains(&RequestEditorEvent::Changed));
}

#[gpui_kit::test]
fn form_data_exposes_file_picker_for_each_file_field(cx: &mut TestAppContext) {
    let mut spec = sample();
    spec.body.kind = BodyKind::Multipart;
    spec.body.form.push(KeyValue::new("attachment", ""));
    let (view, window, _) = open(cx, spec);

    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("form-add-file").is_some());
        w.within(("form-type", 0usize)).click("file", cx);
        w.render_frame(cx);
        assert!(
            w.within("req-form")
                .try_find(("form-pick-file", 0usize))
                .is_some()
        );
    });
    assert!(view.read_with(window, |v, _| v.spec().body.form[0].file));

    window.update(|w, cx| {
        w.within(("form-type", 0usize)).click("text", cx);
    });
    assert!(!view.read_with(window, |v, _| v.spec().body.form[0].file));
}

#[gpui_kit::test]
fn editing_existing_param_updates_spec(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx, sample());
    window.update(|w, cx| {
        w.within("req-params").click(("value", 0usize), cx);
        w.press("cmd-a", cx);
        w.input("25", cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |v, _| {
        assert_eq!(v.spec().params[0], KeyValue::new("limit", "25"));
        assert_eq!(v.spec().params.len(), 2);
    });
}

#[gpui_kit::test]
fn toggle_and_delete_rows(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx, sample());
    window.update(|w, cx| w.within("req-params").click(("enabled", 0usize), cx));
    view.read_with(window, |v, _| {
        assert!(!v.spec().params[0].enabled);
        assert_eq!(v.count(RequestEditorTab::Params), 0);
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        w.within("req-params").click(("delete", 0usize), cx);
    });
    view.read_with(window, |v, _| {
        assert_eq!(v.spec().params.len(), 1);
        assert_eq!(v.spec().params[0].key, "page");
    });
    // The remaining row's editors moved up with it.
    window.update(|w, cx| {
        w.render_frame(cx);
        w.within("req-params").click(("key", 0usize), cx);
        w.press("cmd-right", cx);
        w.input("s", cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    assert_eq!(
        view.read_with(window, |v, _| v.spec().params[0].key.clone()),
        "pages"
    );
}

#[gpui_kit::test]
fn body_mode_and_beautify(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx, sample());
    window.update(|w, cx| {
        w.click(("subtab", 2usize), cx);
        w.render_frame(cx);
        assert!(w.try_find("beautify").is_none());
        assert!(
            w.try_find("generate-body").is_none(),
            "not linked to a spec"
        );
        w.within("body-mode").click(3usize, cx);
    });
    assert_eq!(
        view.read_with(window, |v, _| v.spec().body.kind),
        BodyKind::Json
    );

    window.update(|w, cx| {
        view.update(cx, |v, cx| {
            v.set_body_text("{\"b\":1,\"a\":[true]}".into(), w, cx)
        });
        w.render_frame(cx);
        w.click("beautify", cx);
    });
    view.read_with(window, |v, _| {
        assert_eq!(
            v.spec().body.text,
            "{\n  \"b\": 1,\n  \"a\": [\n    true\n  ]\n}"
        );
        assert!(v.body_error().is_none());
    });

    window.update(|w, cx| {
        view.update(cx, |v, cx| v.set_body_text("{oops".into(), w, cx));
        w.render_frame(cx);
        w.click("beautify", cx);
        w.render_frame(cx);
        assert!(w.try_find("body-error").is_some());
    });
    view.read_with(window, |v, _| {
        assert!(v.body_error().unwrap().starts_with("Невалидный JSON:"));
        assert_eq!(v.spec().body.text, "{oops");
    });

    // Text mode, then Form mode with its own editable rows.
    window.update(|w, cx| {
        w.within("body-mode").click(4usize, cx);
        w.render_frame(cx);
        assert!(w.try_find("body-error").is_none());
        w.within("body-mode").click(2usize, cx);
        w.render_frame(cx);
        w.within("req-form").click(("key", 0usize), cx);
        w.input("email", cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |v, _| {
        assert_eq!(v.spec().body.kind, BodyKind::Form);
        assert_eq!(v.spec().body.form, vec![KeyValue::new("email", "")]);
    });
}

#[gpui_kit::test]
fn auth_type_switch_and_bearer(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx, sample());
    window.update(|w, cx| {
        w.click(("subtab", 3usize), cx);
        w.render_frame(cx);
        w.click("auth-type", cx);
        w.render_frame(cx);
        w.click(("auth-option", 1usize), cx); // Bearer
        w.render_frame(cx);
        // Empty token: the input is shown so it can be typed into.
        w.click("auth-token", cx);
        w.input("eyJhbGciOiJIUzI1NiJ9.e30.abc3fQ", cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    assert_eq!(
        view.read_with(window, |v, _| v.spec().auth.clone()),
        Auth::Bearer {
            token: "eyJhbGciOiJIUzI1NiJ9.e30.abc3fQ".into()
        }
    );

    // Reloading masks the token until the eye is clicked.
    window.update(|w, cx| {
        let spec = view.read(cx).spec().clone();
        view.update(cx, |v, cx| v.set_spec(spec, w, cx));
        w.render_frame(cx);
        assert!(w.try_find("auth-token").is_none());
        assert!(w.try_find("auth-token-masked").is_some());
        w.click("auth-token-eye", cx);
        w.render_frame(cx);
        assert!(w.try_find("auth-token").is_some());
    });

    window.update(|w, cx| {
        w.click("auth-type", cx);
        w.render_frame(cx);
        w.click(("auth-option", 3usize), cx); // API Key
        w.render_frame(cx);
        w.click("auth-api-key", cx);
        w.input("X-Key", cx);
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        w.within("auth-api-in").click(1usize, cx);
    });
    assert_eq!(
        view.read_with(window, |v, _| v.spec().auth.clone()),
        Auth::ApiKey {
            key: "X-Key".into(),
            value: String::new(),
            in_query: true
        }
    );

    window.update(|w, cx| {
        w.render_frame(cx);
        w.click("auth-type", cx);
        w.render_frame(cx);
        w.click(("auth-option", 4usize), cx); // No auth
    });
    assert_eq!(
        view.read_with(window, |v, _| v.spec().auth.clone()),
        Auth::None
    );
}

#[gpui_kit::test]
fn generate_body_emitted_only_when_linked(cx: &mut TestAppContext) {
    let mut spec = sample();
    spec.spec_ref = Some(SpecRef {
        method: Method::Post,
        path: "/orders".into(),
    });
    let (view, window, events) = open(cx, spec);
    window.update(|w, cx| {
        w.click(("subtab", 2usize), cx);
        w.render_frame(cx);
        w.click("generate-body", cx);
    });
    assert!(events.borrow().contains(&RequestEditorEvent::GenerateBody));

    window.update(|w, cx| {
        view.update(cx, |v, cx| v.set_body_text("{\"id\": 1}".into(), w, cx));
    });
    view.read_with(window, |v, _| {
        assert_eq!(v.spec().body.kind, BodyKind::Json);
        assert_eq!(v.spec().body.text, "{\"id\": 1}");
    });
}

#[gpui_kit::test]
fn set_spec_rebuilds_rows(cx: &mut TestAppContext) {
    let (view, window, events) = open(cx, sample());
    let mut other = RequestSpec::new("Other", Method::Get, "/x");
    other.params = vec![
        KeyValue::new("a", "1"),
        KeyValue::new("b", "{{v}}"),
        KeyValue::new("c", "3"),
    ];
    other.auth = Auth::Basic {
        username: "u".into(),
        password: "p".into(),
    };
    window.update(|w, cx| {
        view.update(cx, |v, cx| v.set_spec(other.clone(), w, cx));
        w.render_frame(cx);
        assert!(
            w.within("req-params").try_find(("key", 3usize)).is_some(),
            "3 rows + draft"
        );
        assert!(w.within("req-params").try_find(("key", 4usize)).is_none());
    });
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |v, _| assert_eq!(v.spec(), &other));
    assert!(events.borrow().is_empty(), "set_spec emits nothing");

    // Editing after a rebuild targets the new rows.
    window.update(|w, cx| {
        w.within("req-params").click(("key", 2usize), cx);
        w.press("cmd-right", cx);
        w.input("c", cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |v, _| {
        assert_eq!(v.spec().params[2].key, "cc");
        assert_eq!(v.spec().params[1].value, "{{v}}");
    });
}

#[gpui_kit::test]
fn scripts_tab_typing_updates_pre_request(cx: &mut TestAppContext) {
    let (view, window, events) = open(cx, sample());
    window.update(|w, cx| {
        assert!(w.try_find("scripts-dot").is_none());
        w.click(("subtab", RequestEditorTab::Scripts as usize), cx);
        w.render_frame(cx);
        assert!(w.try_find("script-stage").is_some());
        assert!(w.try_find("script-hint").is_some());
        assert!(w.try_find(("script-snippet", 0usize)).is_some());
        w.click("script-editor", cx);
        w.input("console.log(1);", cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |v, _| {
        assert_eq!(v.active_tab(), RequestEditorTab::Scripts);
        assert_eq!(v.script_stage(), ScriptStage::PreRequest);
        assert_eq!(v.spec().scripts.pre_request, "console.log(1);");
        assert!(v.spec().scripts.post_response.is_empty());
        assert!(v.has_scripts());
    });
    assert!(events.borrow().contains(&RequestEditorEvent::Changed));
    window.update(|w, _| {
        assert!(w.try_find("scripts-dot").is_some());
        assert!(
            w.within("script-stage")
                .try_find(("script-stage-dot", 0usize))
                .is_some()
        );
        assert!(
            w.within("script-stage")
                .try_find(("script-stage-dot", 1usize))
                .is_none()
        );
    });
}

#[gpui_kit::test]
fn scripts_post_response_snippet(cx: &mut TestAppContext) {
    let (view, window, events) = open(cx, sample());
    window.update(|w, cx| {
        w.click(("subtab", RequestEditorTab::Scripts as usize), cx);
        w.render_frame(cx);
        w.within("script-stage").click(1usize, cx);
        w.render_frame(cx);
        w.click(("script-snippet", 0usize), cx);
    });
    view.read_with(window, |v, _| {
        assert_eq!(v.script_stage(), ScriptStage::PostResponse);
        assert!(
            v.spec()
                .scripts
                .post_response
                .contains("pm.response.json()")
        );
        assert!(v.spec().scripts.pre_request.is_empty());
    });
    assert!(events.borrow().contains(&RequestEditorEvent::Changed));

    // A second snippet goes on its own line after the first.
    window.update(|w, cx| {
        view.update(cx, |v, cx| v.insert_snippet(1, w, cx));
    });
    view.read_with(window, |v, _| {
        let post = &v.spec().scripts.post_response;
        assert!(post.starts_with("const json = pm.response.json();"));
        assert!(post.contains(");\npm.test(\"Status is 200\""));
    });

    // Typing into the post-response editor edits that script only.
    window.update(|w, cx| {
        w.render_frame(cx);
        w.click("script-editor", cx);
        w.press("cmd-a", cx);
        w.input("x", cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |v, _| {
        assert_eq!(v.spec().scripts.post_response, "x");
        assert!(v.spec().scripts.pre_request.is_empty());
    });
}

#[gpui_kit::test]
fn set_spec_loads_scripts(cx: &mut TestAppContext) {
    let (view, window, events) = open(cx, sample());
    let mut other = sample();
    other.scripts = Scripts {
        pre_request: "pm.environment.set(\"a\", \"1\");".into(),
        post_response: "console.log(2);".into(),
    };
    window.update(|w, cx| {
        view.update(cx, |v, cx| {
            v.set_spec(other.clone(), w, cx);
            v.set_tab(RequestEditorTab::Scripts, cx);
        });
        w.render_frame(cx);
        assert!(w.try_find("scripts-dot").is_some());
        assert!(
            w.within("script-stage")
                .try_find(("script-stage-dot", 1usize))
                .is_some()
        );
    });
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |v, _| assert_eq!(v.spec(), &other));
    assert!(events.borrow().is_empty(), "set_spec emits nothing");

    // Both editors hold the loaded text: appending keeps the rest.
    window.update(|w, cx| {
        w.click("script-editor", cx);
        w.press("cmd-end", cx);
        w.input("//", cx);
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        view.update(cx, |v, cx| {
            v.set_script_stage(ScriptStage::PostResponse, cx)
        });
        w.render_frame(cx);
        w.click("script-editor", cx);
        w.press("cmd-end", cx);
        w.input("//", cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |v, _| {
        assert_eq!(
            v.spec().scripts.pre_request,
            "pm.environment.set(\"a\", \"1\");//"
        );
        assert_eq!(v.spec().scripts.post_response, "console.log(2);//");
    });
}

#[gpui_kit::test]
fn bulk_edit_round_trips_params_and_headers(cx: &mut TestAppContext) {
    let mut spec = sample();
    spec.params[0].description = "page size".into();
    let (view, window, events) = open(cx, spec);
    window.update(|w, cx| {
        w.within("req-params").click("bulk-toggle", cx);
        w.render_frame(cx);
        assert!(w.within("req-params").try_find("bulk-editor").is_some());
        assert!(w.within("req-params").try_find(("key", 0usize)).is_none());
    });
    view.read_with(window, |v, cx| {
        assert!(v.is_bulk_edit(RequestEditorTab::Params));
        assert!(!v.is_bulk_edit(RequestEditorTab::Headers));
        assert_eq!(
            v.bulk_text(RequestEditorTab::Params, cx).as_deref(),
            Some("limit:10\n//page:1")
        );
    });
    assert!(events.borrow().is_empty(), "opening bulk edit is not a change");

    // Replace the text (Postman format): order, `//` = disabled.
    window.update(|w, cx| {
        w.click("bulk-editor", cx);
        w.press("cmd-a", cx);
        w.input(
            "access_token:{{access_token}}\nv:{{v}}\n//restore_sid:\nnew_password:\nlimit:20",
            cx,
        );
    });
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |v, _| {
        let got: Vec<(bool, &str, &str)> = v
            .spec()
            .params
            .iter()
            .map(|kv| (kv.enabled, kv.key.as_str(), kv.value.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                (true, "access_token", "{{access_token}}"),
                (true, "v", "{{v}}"),
                (false, "restore_sid", ""),
                (true, "new_password", ""),
                (true, "limit", "20"),
            ]
        );
        // The description survives text edits of the same key.
        assert_eq!(v.spec().params[4].description, "page size");
    });
    assert!(events.borrow().contains(&RequestEditorEvent::Changed));

    // Back to the table: rows rebuilt from the edited list (+ draft row).
    window.update(|w, cx| {
        w.within("req-params").click("bulk-toggle", cx);
        w.render_frame(cx);
        assert!(w.within("req-params").try_find("bulk-editor").is_none());
        assert!(w.within("req-params").try_find(("key", 4usize)).is_some());
        assert!(w.within("req-params").try_find(("key", 5usize)).is_some());
        assert!(w.within("req-params").try_find(("key", 6usize)).is_none());
    });
    view.read_with(window, |v, _| {
        assert!(!v.is_bulk_edit(RequestEditorTab::Params));
        assert_eq!(v.count(RequestEditorTab::Params), 4);
    });

    // Headers have their own toggle state.
    window.update(|w, cx| {
        w.click(("subtab", RequestEditorTab::Headers as usize), cx);
        w.render_frame(cx);
        w.within("req-headers").click("bulk-toggle", cx);
        w.render_frame(cx);
        assert!(w.within("req-headers").try_find("bulk-editor").is_some());
    });
    view.read_with(window, |v, cx| {
        assert_eq!(
            v.bulk_text(RequestEditorTab::Headers, cx).as_deref(),
            Some("Accept:application/json")
        );
    });
}

#[gpui_kit::test]
fn invalid_headers_are_highlighted_with_a_hint(cx: &mut TestAppContext) {
    let mut spec = sample();
    spec.headers = vec![
        KeyValue::new("Accept", "application/json"),
        KeyValue::new("X Token", "1"),
        KeyValue::new("Cookie", "a=1\nb=2"),
        KeyValue::new("X-Pad", " v "),
        KeyValue::new("{{hdr}}", "{{value}}"),
    ];
    let (view, window, _) = open(cx, spec);
    window.update(|w, cx| {
        w.click(("subtab", RequestEditorTab::Headers as usize), cx);
        w.render_frame(cx);
        let table = w.within("req-headers");
        assert!(table.try_find(("key-issue", 0usize)).is_none());
        assert!(table.try_find(("value-issue", 0usize)).is_none());
        assert!(table.try_find(("key-issue", 1usize)).is_some());
        assert!(table.try_find(("value-issue", 2usize)).is_some());
        assert!(table.try_find(("value-issue", 3usize)).is_some());
        // Placeholders are not judged before substitution.
        assert!(table.try_find(("key-issue", 4usize)).is_none());
        assert!(table.try_find(("value-issue", 4usize)).is_none());
    });
    view.read_with(window, |v, _| {
        let (name, _) = v.header_issues(1);
        assert_eq!(name.unwrap().message, "В имени заголовка не может быть пробелов");
        let (_, value) = v.header_issues(2);
        let value = value.unwrap();
        assert!(value.is_error());
        assert_eq!(value.message, "В заголовке не может быть перевода строки");
        let (_, pad) = v.header_issues(3);
        assert!(!pad.unwrap().is_error(), "edge blanks are only a warning");
    });
    // Fixing the name in the table clears the mark.
    window.update(|w, cx| {
        w.within("req-headers").click(("key", 1usize), cx);
        w.press("cmd-a", cx);
        w.input("X-Token", cx);
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.within("req-headers").try_find(("key-issue", 1usize)).is_none());
    });
}

#[gpui_kit::test]
fn long_value_cell_expands_while_focused(cx: &mut TestAppContext) {
    let long = "session=8f14e45fceea167a5a36dedd4bea2543; csrftoken=Zx9Kq2LmN8pR4tV7wY1aB3cD5eF6gH0j; theme=dark; lang=ru; ".repeat(4);
    let long = long.as_str();
    let mut spec = sample();
    spec.headers = vec![KeyValue::new("Cookie", long), KeyValue::new("Accept", "*/*")];
    let (view, window, events) = open(cx, spec);
    window.update(|w, cx| {
        w.click(("subtab", RequestEditorTab::Headers as usize), cx);
        w.render_frame(cx);
        // A short value stays a one-line cell.
        w.within("req-headers").click(("value", 1usize), cx);
        w.render_frame(cx);
    });
    window.run_until_parked();
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("value-expanded").is_none());
        w.within("req-headers").click(("value", 0usize), cx);
        w.render_frame(cx);
    });
    window.run_until_parked();
    window.update(|w, cx| {
        w.render_frame(cx);
        let expanded = w.find("value-expanded").bounds();
        let cell = w.within("req-headers").find(("value-cell", 0usize)).bounds();
        // Same width as the cell, taller (wrapped), over the rows below.
        assert!((expanded.size.width.as_f32() - cell.size.width.as_f32()).abs() < 30.);
        assert!(expanded.size.height.as_f32() > cell.size.height.as_f32() + 10.);
        assert!(expanded.size.height.as_f32() <= 200.5);
    });
    assert!(view.read_with(window, |v, _| v.has_expanded_value()));

    // Editing in the expanded editor updates the value.
    window.update(|w, cx| {
        w.press("cmd-a", cx);
        w.input("short=1", cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |v, _| assert_eq!(v.spec().headers[0].value, "short=1"));
    assert!(events.borrow().contains(&RequestEditorEvent::Changed));

    // Leaving the field folds it back into the cell with the new text.
    window.update(|w, cx| {
        w.within("req-headers").click(("key", 1usize), cx);
        w.render_frame(cx);
    });
    window.run_until_parked();
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("value-expanded").is_none());
    });
    view.read_with(window, |v, cx| {
        assert!(!v.has_expanded_value());
        assert_eq!(v.spec().headers[0].value, "short=1");
        let _ = cx;
    });
}

#[gpui_kit::test]
fn bulk_edit_in_body_form_table(cx: &mut TestAppContext) {
    let mut spec = sample();
    spec.body.kind = BodyKind::Form;
    spec.body.form = vec![
        KeyValue::new("sdafasdf", "asdfdfas"),
        KeyValue::new("asads", "Asdsda").enabled(false),
    ];
    let (view, window, _) = open(cx, spec);
    window.update(|w, cx| {
        w.render_frame(cx);
        // The toggle sits in the form table's header, right-aligned.
        let header_toggle = w.within("req-form").find("bulk-toggle").bounds();
        let table = w.find("request-body-pane").bounds();
        let right = (header_toggle.origin.x + header_toggle.size.width).as_f32();
        assert!(((table.origin.x + table.size.width).as_f32() - right).abs() < 20.);
        w.within("req-form").click("bulk-toggle", cx);
        w.render_frame(cx);
        assert!(w.within("req-form").try_find("bulk-editor").is_some());
        // The Params table above keeps its rows.
        assert!(w.within("req-params").try_find("bulk-editor").is_none());
    });
    view.read_with(window, |v, cx| {
        assert!(v.is_bulk_edit(RequestEditorTab::Body));
        assert!(!v.is_bulk_edit(RequestEditorTab::Params));
        assert_eq!(
            v.bulk_text(RequestEditorTab::Body, cx).as_deref(),
            Some("sdafasdf:asdfdfas\n//asads:Asdsda")
        );
    });
    window.update(|w, cx| {
        w.within("req-form").click("bulk-editor", cx);
        w.press("cmd-a", cx);
        w.input("a:1\n//b:2", cx);
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        w.within("req-form").click("bulk-toggle", cx);
        w.render_frame(cx);
        assert!(w.within("req-form").try_find(("key", 1usize)).is_some());
    });
    view.read_with(window, |v, _| {
        let form = &v.spec().body.form;
        assert_eq!(form.len(), 2);
        assert_eq!((form[0].key.as_str(), form[0].value.as_str(), form[0].enabled), ("a", "1", true));
        assert_eq!((form[1].key.as_str(), form[1].value.as_str(), form[1].enabled), ("b", "2", false));
    });
}

#[gpui_kit::test]
fn path_variables_follow_the_url(cx: &mut TestAppContext) {
    let mut spec = sample();
    spec.url = "{{baseUrl}}/users/:id/posts/:post".into();
    spec.path_vars = vec![KeyValue::new("id", "7").description("user id")];
    let (view, window, events) = open(cx, spec);
    window.update(|w, cx| {
        w.render_frame(cx);
        let table = w.within("req-path-vars");
        assert!(table.try_find(("value", 0usize)).is_some());
        assert!(table.try_find(("value", 1usize)).is_some());
        assert!(table.try_find(("value", 2usize)).is_none(), "no draft row");
        assert!(table.try_find(("key", 0usize)).is_none(), "names are not editable");
        assert!(table.try_find(("delete", 0usize)).is_none());
    });
    view.read_with(window, |v, _| {
        assert_eq!(
            v.spec().path_vars,
            vec![KeyValue::new("id", "7").description("user id"), KeyValue::new("post", "")]
        );
    });
    assert!(events.borrow().is_empty(), "loading a request is not an edit");

    // Typing a value.
    window.update(|w, cx| {
        w.within("req-path-vars").click(("value", 1usize), cx);
        w.input("99", cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |v, _| assert_eq!(v.spec().path_vars[1].value, "99"));

    // The URL loses `:post` and gains `:tag`: values of kept names survive.
    view.update_in(window, |v, window, cx| v.set_url("{{baseUrl}}/users/:id/tags/:tag", window, cx));
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |v, _| {
        assert_eq!(
            v.spec().path_vars,
            vec![KeyValue::new("id", "7").description("user id"), KeyValue::new("tag", "")]
        );
    });
    assert!(events.borrow().contains(&RequestEditorEvent::Changed));
    // No `:name` left: the section disappears.
    view.update_in(window, |v, window, cx| v.set_url("{{baseUrl}}/users", window, cx));
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("params-with-path").is_none());
    });
}

#[gpui_kit::test]
fn docs_tab_edits_the_description(cx: &mut TestAppContext) {
    let mut spec = sample();
    spec.description = "Creates an order.".into();
    let (view, window, events) = open(cx, spec);
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("docs-dot").is_some(), "a described request is marked");
        w.click(("subtab", RequestEditorTab::Docs as usize), cx);
        w.render_frame(cx);
        assert!(w.try_find("request-docs").is_some());
        w.click("request-docs", cx);
        w.press("cmd-a", cx);
        w.input("Lists *all* orders.", cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |v, _| {
        assert_eq!(v.active_tab(), RequestEditorTab::Docs);
        assert_eq!(v.spec().description, "Lists *all* orders.");
    });
    assert!(events.borrow().contains(&RequestEditorEvent::Changed));
    // set_spec loads another request's docs without an edit event.
    events.borrow_mut().clear();
    let mut other = sample();
    other.description = String::new();
    view.update_in(window, |v, window, cx| v.set_spec(other, window, cx));
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("docs-dot").is_none());
    });
    view.read_with(window, |v, _| assert!(v.spec().description.is_empty()));
}

#[gpui_kit::test]
fn request_connection_settings_override_ssl_and_timeout(cx: &mut TestAppContext) {
    let (view, window, events) = open(cx, sample());
    window.update(|w, cx| {
        w.click(("subtab", RequestEditorTab::Settings as usize), cx);
        w.render_frame(cx);
        assert!(w.try_find("request-connection-settings").is_some());
        w.within("request-ssl-verify").click(2usize, cx);
        w.click("request-timeout", cx);
        w.input("2500", cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |editor, _| {
        assert_eq!(editor.spec().ssl_verify, Some(false));
        assert_eq!(editor.spec().timeout_ms, Some(2500));
    });
    assert!(events.borrow().contains(&RequestEditorEvent::Changed));
    // Out of range is ignored; empty and "По умолчанию" fall back to the app.
    window.update(|w, cx| {
        w.click("request-timeout", cx);
        w.press("cmd-a", cx);
        w.input("0", cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |editor, _| assert_eq!(editor.spec().timeout_ms, Some(2500)));
    window.update(|w, cx| {
        w.press("backspace", cx);
        w.within("request-ssl-verify").click(0usize, cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    view.read_with(window, |editor, _| {
        assert_eq!(editor.spec().timeout_ms, None);
        assert_eq!(editor.spec().ssl_verify, None);
    });
}
