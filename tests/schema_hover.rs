//! Schema popovers (request body, response Pretty, params) and live JSON
//! validation of the request body.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::highlighter::DiagnosticSeverity;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    Entity, InputEvent as _, MouseMoveEvent, TestAppContext, VisualTestContext, point, px,
};
use rivet::app::request_editor::RequestEditor;
use rivet::app::response_view::{ResponseState, ResponseView};
use rivet::core::openapi::Spec;
use rivet::core::schema_hover::SchemaLink;
use rivet::core::types::{
    BodyKind, KeyValue, Mismatch, RequestSpec, ResponseData, SpecRef, Timings,
};
use rivet::model::Method;
use rivet::theme::Palette;

const ORDER_BODY: &str = "{\n  \"customerId\": \"cus_1001\",\n  \"items\": [\n    {\n      \"productId\": \"prd_42\",\n      \"quantity\": 2\n    }\n  ]\n}";

fn shop() -> Arc<Spec> {
    Arc::new(
        Spec::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/shop-api.yaml"))
            .unwrap(),
    )
}

fn link(method: Method, path: &str) -> SchemaLink {
    SchemaLink::new(
        shop(),
        SpecRef {
            method,
            path: path.into(),
        },
    )
}

fn order_request(body: &str) -> RequestSpec {
    let mut spec = RequestSpec::new("Create order", Method::Post, "{{baseUrl}}/orders");
    spec.body.kind = BodyKind::Json;
    spec.body.text = body.into();
    spec
}

fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        Palette::dark().apply(cx);
    });
}

fn open_editor(
    cx: &mut TestAppContext,
    spec: RequestSpec,
    schema: Option<SchemaLink>,
) -> (Entity<RequestEditor>, &mut VisualTestContext) {
    init(cx);
    let (view, window) = cx.add_window_view(|window, cx| RequestEditor::new(spec, window, cx));
    window.update(|w, cx| {
        view.update(cx, |e, cx| e.set_schema_link(schema, cx));
        w.render_frame(cx);
        w.render_frame(cx);
    });
    (view, window)
}

fn label(w: &gpui_kit::Window, id: &'static str) -> String {
    w.find(id).label().unwrap_or_default().to_string()
}

#[gpui_kit::test]
fn pointer_resting_on_a_body_key_shows_the_schema_popover(cx: &mut TestAppContext) {
    let (view, window) = open_editor(cx, order_request(ORDER_BODY), Some(link(Method::Post, "/orders")));
    let key = ORDER_BODY.find("\"quantity\"").unwrap();
    // Real pointer path: mouse move over the key → editor hover provider (after its
    // delay) → popover.
    let target = view.read_with(window, |e, cx| {
        e.body_editor()
            .read(cx)
            .range_to_bounds(&(key + 2..key + 3))
            .expect("key laid out")
            .center()
    });
    window.update(|w, cx| {
        w.dispatch_event(
            MouseMoveEvent {
                position: target,
                pressed_button: None,
                modifiers: Default::default(),
            }
            .to_platform_input(),
            cx,
        );
        w.render_frame(cx);
    });
    window.executor().advance_clock(Duration::from_millis(300));
    window.run_until_parked();
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("field-hover").is_some(), "popover shown");
        assert_eq!(label(w, "field-hover"), "quantity");
        assert_eq!(label(w, "field-hover-type"), "integer");
        assert_eq!(label(w, "field-hover-required"), "required");
        assert_eq!(label(w, "field-hover-constraints"), "min: 1");
        assert_eq!(
            label(w, "field-hover-description"),
            "Количество единиц товара в позиции заказа"
        );
        assert_eq!(label(w, "field-hover-path"), "OrderItem.quantity");
        assert!(w.try_find("field-hover-mismatch").is_none());
    });
    // Moving onto whitespace hides it.
    let blank = view.read_with(window, |e, cx| {
        let b = e.body_editor().read(cx).range_to_bounds(&(1..2)).unwrap();
        point(b.left() + px(200.), b.center().y)
    });
    window.update(|w, cx| {
        w.dispatch_event(
            MouseMoveEvent {
                position: blank,
                pressed_button: None,
                modifiers: Default::default(),
            }
            .to_platform_input(),
            cx,
        );
    });
    window.executor().advance_clock(Duration::from_millis(300));
    window.run_until_parked();
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("field-hover").is_none());
    });
}

#[gpui_kit::test]
fn value_hover_and_optional_fields(cx: &mut TestAppContext) {
    let (view, window) = open_editor(cx, order_request(ORDER_BODY), Some(link(Method::Post, "/orders")));
    let value = ORDER_BODY.find("\"cus_1001\"").unwrap() + 1;
    window.update(|w, cx| {
        view.update(cx, |e, cx| e.hover_body_offset(value, cx));
        w.render_frame(cx);
        assert_eq!(label(w, "field-hover"), "customerId");
        assert_eq!(label(w, "field-hover-type"), "string");
        assert_eq!(label(w, "field-hover-path"), "OrderCreate.customerId");
    });
    let items = ORDER_BODY.find("\"items\"").unwrap() + 1;
    window.update(|w, cx| {
        view.update(cx, |e, cx| e.hover_body_offset(items, cx));
        w.render_frame(cx);
        assert_eq!(label(w, "field-hover-type"), "OrderItem[]");
        assert_eq!(label(w, "field-hover-constraints"), "minItems: 1");
    });
    // Whitespace: nothing.
    window.update(|w, cx| {
        view.update(cx, |e, cx| e.hover_body_offset(1, cx));
        w.render_frame(cx);
        assert!(w.try_find("field-hover").is_none());
    });
}

#[gpui_kit::test]
fn unlinked_request_has_no_popover(cx: &mut TestAppContext) {
    let (view, window) = open_editor(cx, order_request(ORDER_BODY), None);
    let key = ORDER_BODY.find("\"quantity\"").unwrap() + 1;
    window.update(|w, cx| {
        view.update(cx, |e, cx| e.hover_body_offset(key, cx));
        w.render_frame(cx);
        assert!(w.try_find("field-hover").is_none());
    });
    assert!(view.read_with(window, |e, _| e.field_hover().is_none()));
}

#[gpui_kit::test]
fn params_key_cell_hover_shows_param_info(cx: &mut TestAppContext) {
    let mut spec = RequestSpec::new("List orders", Method::Get, "{{baseUrl}}/orders");
    spec.params = vec![KeyValue::new("limit", "20"), KeyValue::new("custom", "1")];
    let (_, window) = open_editor(cx, spec, Some(link(Method::Get, "/orders")));
    window.update(|w, cx| {
        w.within("req-params").hover(("key-cell", 0usize), cx);
        w.render_frame(cx);
        assert_eq!(label(w, "field-hover"), "limit");
        assert_eq!(label(w, "field-hover-type"), "integer");
        assert_eq!(label(w, "field-hover-location"), "query");
        assert!(w.try_find("field-hover-required").is_none());
        assert_eq!(label(w, "field-hover-constraints"), "min: 1  max: 100  default: 20");
        assert_eq!(
            label(w, "field-hover-description"),
            "Максимальное количество заказов на странице"
        );
        // A param the spec doesn't declare: no popover.
        w.within("req-params").hover(("key-cell", 1usize), cx);
        w.render_frame(cx);
        assert!(w.try_find("field-hover").is_none());
    });
}

// ---- live validation --------------------------------------------------------

#[gpui_kit::test]
fn typing_invalid_json_sets_an_error_diagnostic(cx: &mut TestAppContext) {
    let (view, window) = open_editor(cx, order_request(""), None);
    assert_eq!(view.read_with(window, |e, _| e.body_summary()), None);
    window.update(|w, cx| {
        w.click("body-editor", cx);
        w.input("1 2", cx);
    });
    window.update(|w, cx| w.render_frame(cx));
    let diagnostics = view.read_with(window, |e, cx| e.body_diagnostics(cx));
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    let (range, severity, message) = &diagnostics[0];
    assert_eq!(*range, 2..3);
    assert_eq!(*severity, DiagnosticSeverity::Error);
    assert!(message.starts_with("Лишние символы после JSON"), "{message}");
    assert!(message.ends_with("(стр. 1, кол. 3)"), "{message}");
    window.update(|w, _| assert_eq!(label(w, "body-validation"), "✗ 1 ошибка"));

    // Clicking the summary puts the caret on the problem.
    window.update(|w, cx| {
        w.click("body-validation", cx);
        w.render_frame(cx);
    });
    assert_eq!(
        view.read_with(window, |e, cx| e.body_editor().read(cx).selected_range()),
        2..2
    );

    // Errors map to the offending token.
    window.update(|w, cx| {
        view.update(cx, |e, cx| {
            e.set_body_text("{\n  \"a\": 1\n  \"b\": 2\n}".into(), w, cx)
        });
        w.render_frame(cx);
    });
    let diagnostics = view.read_with(window, |e, cx| e.body_diagnostics(cx));
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].0, 13..16);
    assert!(diagnostics[0].2.starts_with("Ожидается `,` или `}`"));
}

#[gpui_kit::test]
fn placeholders_are_valid_json(cx: &mut TestAppContext) {
    let body = r#"{"customerId": "{{customerId}}", "n": {{count}}}"#;
    let (view, window) = open_editor(cx, order_request(body), None);
    assert!(view.read_with(window, |e, cx| e.body_diagnostics(cx)).is_empty());
    window.update(|w, _| assert_eq!(label(w, "body-validation"), "✓ Валидный JSON"));
}

#[gpui_kit::test]
fn schema_warning_for_wrong_type_on_linked_request(cx: &mut TestAppContext) {
    let body = ORDER_BODY.replace("\"quantity\": 2", "\"quantity\": \"2\"");
    let (view, window) = open_editor(cx, order_request(&body), Some(link(Method::Post, "/orders")));
    let diagnostics = view.read_with(window, |e, cx| e.body_diagnostics(cx));
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    let key = body.find("\"quantity\"").unwrap();
    assert_eq!(diagnostics[0].0, key..key + "\"quantity\"".len());
    assert_eq!(diagnostics[0].1, DiagnosticSeverity::Warning);
    assert!(diagnostics[0].2.contains("integer"), "{}", diagnostics[0].2);
    window.update(|w, cx| {
        assert_eq!(label(w, "body-validation"), "⚠ 1 предупреждение по схеме");
        // The hover popover carries the mismatch too.
        view.update(cx, |e, cx| e.hover_body_offset(key + 1, cx));
        w.render_frame(cx);
        assert!(label(w, "field-hover-mismatch").contains("Получено"));
    });
    // Unlinking drops the schema warning.
    window.update(|w, cx| {
        view.update(cx, |e, cx| e.set_schema_link(None, cx));
        w.render_frame(cx);
        assert_eq!(label(w, "body-validation"), "✓ Валидный JSON");
    });
}

// ---- response view ----------------------------------------------------------

fn response(body: &str) -> ResponseData {
    ResponseData {
        status: 201,
        status_text: "Created".into(),
        http_version: "HTTP/1.1".into(),
        remote_addr: None,
        headers: vec![("content-type".into(), "application/json".into())],
        redirect_history: Vec::new(),
        hops: Vec::new(),
        body: body.as_bytes().to_vec(),
        timings: Timings {
            redirect: Duration::ZERO,
            dns: Duration::ZERO,
            connect: Duration::ZERO,
            tls: Duration::ZERO,
            ttfb: Duration::from_millis(10),
            download: Duration::ZERO,
            total: Duration::from_millis(10),
        },
    }
}

#[gpui_kit::test]
fn response_pretty_hover_shows_schema_and_mismatch(cx: &mut TestAppContext) {
    init(cx);
    let (view, window) = cx.add_window_view(ResponseView::new);
    let body = r#"{"id":"ord_1","status":"lost","customerId":"c","items":[],"total":1,"createdAt":"2026-10-15T12:30:00Z"}"#;
    let state = ResponseState::Done {
        response: response(body),
        schema_label: Some("responses.201 → $ref Order".into()),
        mismatches: vec![Mismatch {
            path: "$.status".into(),
            title: "Значение не из enum".into(),
            expected: "pending | paid | shipped | cancelled".into(),
            actual: "\"lost\"".into(),
            line: Some(2),
        }],
    };
    window.update(|w, cx| {
        view.update(cx, |v, cx| {
            v.set_schema_link(Some(link(Method::Post, "/orders")), cx);
            v.set_state(state, cx);
        });
        w.render_frame(cx);
        w.render_frame(cx);
    });
    let pretty = view.read_with(window, |v, cx| v.pretty_editor().read(cx).text().to_string());
    let status = pretty.find("\"status\"").unwrap() + 1;
    window.update(|w, cx| {
        view.update(cx, |v, cx| v.hover_pretty_offset(status, cx));
        w.render_frame(cx);
        assert_eq!(label(w, "field-hover"), "status");
        assert_eq!(label(w, "field-hover-type"), "string");
        assert_eq!(label(w, "field-hover-required"), "required");
        assert_eq!(
            label(w, "field-hover-constraints"),
            "enum: pending | paid | shipped | cancelled"
        );
        assert_eq!(label(w, "field-hover-path"), "Order.status");
        assert_eq!(
            label(w, "field-hover-mismatch"),
            "Ожидается pending | paid | shipped | cancelled · Получено \"lost\""
        );
    });
    // A field without mismatch: no warn section.
    let id = pretty.find("\"id\"").unwrap() + 1;
    window.update(|w, cx| {
        view.update(cx, |v, cx| v.hover_pretty_offset(id, cx));
        w.render_frame(cx);
        assert_eq!(label(w, "field-hover"), "id");
        assert!(w.try_find("field-hover-mismatch").is_none());
    });
    // Not linked: nothing.
    window.update(|w, cx| {
        view.update(cx, |v, cx| {
            v.set_schema_link(None, cx);
            v.hover_pretty_offset(status, cx);
        });
        w.render_frame(cx);
        assert!(w.try_find("field-hover").is_none());
    });
}
