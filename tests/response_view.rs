use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::test::TestWindowExt;
use gpui_kit::{ElementId, Entity, Focusable as _, TestAppContext, VisualTestContext};
use rivet::app::response_view::{
    BodyMode, LARGE_BODY_BYTES, ResponseState, ResponseTab, ResponseView, ResponseViewEvent,
    pretty_json,
};
use rivet::core::types::{
    LogLevel, LogLine, Method, Mismatch, RedirectHop, ResponseData, ScriptError, ScriptOutcome,
    ScriptReport, SendError, TestResult, Timings,
};
use rivet::theme::Palette;

type Events = Rc<RefCell<Vec<ResponseViewEvent>>>;

fn open(cx: &mut TestAppContext) -> (Entity<ResponseView>, &mut VisualTestContext, Events) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        Palette::dark().apply(cx);
    });
    let (view, window) = cx.add_window_view(ResponseView::new);
    let events: Events = Rc::default();
    let sink = events.clone();
    window.update(|_, cx| {
        cx.subscribe(&view, move |_, ev: &ResponseViewEvent, _| {
            sink.borrow_mut().push(ev.clone())
        })
        .detach();
    });
    window.update(|w, cx| w.render_frame(cx));
    (view, window, events)
}

fn child(parent: impl Into<ElementId>, name: &'static str) -> ElementId {
    ElementId::NamedChild(std::sync::Arc::new(parent.into()), name.into())
}

fn response(body: &str, content_type: &str) -> ResponseData {
    ResponseData {
        status: 201,
        status_text: "Created".into(),
        http_version: "HTTP/2".into(),
        remote_addr: Some("203.0.113.42:443".into()),
        headers: vec![
            ("content-type".into(), content_type.into()),
            ("x-request-id".into(), "req-42".into()),
            ("cache-control".into(), "no-store".into()),
        ],
        redirect_history: Vec::new(),
        hops: Vec::new(),
        body: body.as_bytes().to_vec(),
        timings: Timings {
            redirect: Duration::ZERO,
            dns: Duration::from_millis(12),
            connect: Duration::from_millis(20),
            tls: Duration::from_millis(30),
            ttfb: Duration::from_millis(70),
            download: Duration::from_millis(16),
            total: Duration::from_millis(148),
        },
    }
}

fn order_body() -> String {
    let items: Vec<String> = (0..120)
        .map(|i| format!(r#"{{"sku":"S{i}","quantity":{i}}}"#))
        .collect();
    format!(
        r#"{{"id":"ord_1","status":"shipped","items":[{}],"total":12.5}}"#,
        items.join(",")
    )
}

fn set(view: &Entity<ResponseView>, window: &mut VisualTestContext, state: ResponseState) {
    window.update(|w, cx| {
        view.update(cx, |v, cx| v.set_state(state, cx));
        w.render_frame(cx);
    });
}

fn done(body: String, mismatches: Vec<Mismatch>) -> ResponseState {
    ResponseState::Done {
        response: response(&body, "application/json; charset=utf-8"),
        schema_label: Some("responses.201 → $ref Order".into()),
        mismatches,
    }
}

#[gpui_kit::test]
fn empty_state_by_default(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    assert_eq!(
        view.read_with(window, |v, _| v.state().clone()),
        ResponseState::Empty
    );
    window.update(|w, _| {
        assert!(w.try_find("response-empty").is_some());
        assert!(w.try_find("response-header").is_none());
    });
}

#[gpui_kit::test]
fn done_renders_header_tabs_and_pretty_body(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    set(&view, window, done(order_body(), vec![]));
    window.update(|w, _| {
        for id in [
            "response-header",
            "response-status",
            "response-time",
            "response-size",
            "response-schema",
            "response-bookmark",
        ] {
            assert!(w.try_find(id).is_some(), "{id} missing");
        }
        for id in [
            "tab-body",
            "tab-headers",
            "tab-timeline",
            "body-mode",
            "content-type",
        ] {
            assert!(w.try_find(id).is_some(), "{id} missing");
        }
        assert!(w.try_find("response-pretty").is_some());
        assert!(
            w.try_find("mismatches").is_none(),
            "no mismatch panel without mismatches"
        );
        assert!(w.try_find("large-body").is_none());
    });
    view.read_with(window, |v, _| {
        assert!(v.pretty_available());
        assert_eq!(v.body_mode(), BodyMode::Pretty);
    });
    // The Pretty view is a read-only editor holding the pretty-printed JSON.
    let pretty = pretty_json(order_body().as_bytes()).unwrap();
    assert_eq!(editor_text(&view, window, Body::Pretty), pretty);
    assert!(view.read_with(window, |v, cx| !v.pretty_editor().read(cx).is_editable()));
}

#[gpui_kit::test]
fn pretty_raw_preview_switching(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    set(&view, window, done(order_body(), vec![]));

    window.update(|w, cx| {
        w.within("body-mode").click(1usize, cx);
        w.render_frame(cx);
        assert!(w.try_find("response-raw").is_some());
        assert!(w.try_find("response-pretty").is_none());
    });
    assert_eq!(view.read_with(window, |v, _| v.body_mode()), BodyMode::Raw);
    // Raw is the original single-line body.
    assert_eq!(editor_text(&view, window, Body::Raw), order_body());

    window.update(|w, cx| {
        w.within("body-mode").click(2usize, cx);
        w.render_frame(cx);
        assert!(w.try_find("response-preview").is_some());
        assert!(
            w.try_find("preview-unavailable").is_some(),
            "JSON has no preview"
        );

        w.within("body-mode").click(0usize, cx);
        w.render_frame(cx);
        assert!(w.try_find("response-pretty").is_some());
    });
    assert_eq!(
        view.read_with(window, |v, _| v.body_mode()),
        BodyMode::Pretty
    );
}

#[gpui_kit::test]
fn html_preview_shows_text(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    let state = ResponseState::Done {
        response: response(
            "<html><body><h1>Hi</h1><p>there &amp; back</p></body></html>",
            "text/html",
        ),
        schema_label: None,
        mismatches: vec![],
    };
    set(&view, window, state);
    window.update(|w, cx| {
        assert!(
            w.try_find("response-schema").is_none(),
            "no schema label, no pill"
        );
        w.within("body-mode").click(2usize, cx);
        w.render_frame(cx);
        assert!(w.try_find("response-preview").is_some());
    });
    // "Hi", blank separator, "there & back": tags stripped, blank runs collapsed.
    assert_eq!(
        editor_text(&view, window, Body::Preview),
        "Hi\n\nthere & back"
    );
}

#[gpui_kit::test]
fn mismatch_panel_line_click_highlights_and_scrolls(cx: &mut TestAppContext) {
    let (view, window, events) = open(cx);
    let pretty = pretty_json(order_body().as_bytes()).unwrap();
    let far = pretty.lines().count() - 5;
    let mismatches = vec![
        Mismatch {
            path: "$.status".into(),
            title: "Значение не из enum".into(),
            expected: "enum: pending, paid".into(),
            actual: "\"shipped\"".into(),
            line: Some(2),
        },
        Mismatch {
            path: "$.total".into(),
            title: "Неверный тип".into(),
            expected: "string".into(),
            actual: "number".into(),
            line: Some(far),
        },
        Mismatch {
            path: "$.extra".into(),
            title: "Нет поля".into(),
            expected: "x".into(),
            actual: "—".into(),
            line: None,
        },
    ];
    set(&view, window, done(order_body(), mismatches));

    window.update(|w, _| {
        assert!(w.try_find("mismatches").is_some());
        assert!(w.try_find(("mismatch", 2usize)).is_some());
        assert!(
            w.try_find(("mismatch-line", 2usize)).is_none(),
            "no line → no link"
        );
    });
    let visible = |window: &mut VisualTestContext| {
        view.read_with(window, |v, cx| {
            v.pretty_editor().read(cx).visible_row_range().unwrap()
        })
    };
    assert!(!visible(window).contains(&far), "far line is off-screen");
    // Both mismatches with a line are marked in the editor (warning + note).
    assert_eq!(
        view.read_with(window, |v, cx| {
            v.pretty_editor().read(cx).diagnostics().map_or(0, |d| d.len())
        }),
        2
    );
    window.update(|w, cx| {
        // Switch to Raw first: the link must bring Pretty back.
        w.within("body-mode").click(1usize, cx);
        w.click(("mismatch-line", 1usize), cx);
        w.render_frame(cx);
        w.render_frame(cx);
        assert!(w.try_find("response-pretty").is_some());
    });
    assert!(visible(window).contains(&far), "scrolled to the line");
    // The caret sits at the start of the revealed line.
    let line_start: usize = pretty.lines().take(far).map(|l| l.len() + 1).sum();
    assert_eq!(
        view.read_with(window, |v, cx| v.pretty_editor().read(cx).selected_range()),
        line_start..line_start
    );
    view.read_with(window, |v, _| {
        assert_eq!(v.highlighted_line(), Some(far));
        assert_eq!(v.body_mode(), BodyMode::Pretty);
    });

    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("update-schema").is_none());
        assert!(w.try_find("go-to-schema").is_none());
    });
    assert!(events.borrow().is_empty());
}

#[gpui_kit::test]
fn headers_tab_lists_and_copies(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    set(&view, window, done(order_body(), vec![]));
    window.update(|w, cx| {
        w.click("tab-headers", cx);
        w.render_frame(cx);
        assert!(w.try_find("headers").is_some());
        assert!(w.try_find(("header", 2usize)).is_some());
        assert!(w.try_find(("header", 3usize)).is_none());
        w.click(("header", 1usize), cx);
    });
    assert_eq!(view.read_with(window, |v, _| v.tab()), ResponseTab::Headers);
    let clip = window.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
    assert_eq!(clip.as_deref(), Some("req-42"));
}

#[gpui_kit::test]
fn timeline_has_five_bars(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    set(&view, window, done(order_body(), vec![]));
    window.update(|w, cx| {
        w.click("tab-timeline", cx);
        w.render_frame(cx);
        for id in ["timeline", "timeline-connection", "timeline-total"] {
            assert!(w.try_find(id).is_some(), "{id} missing");
        }
        for i in 0..5usize {
            assert!(w.try_find(("timing-bar", i)).is_some(), "bar {i} missing");
        }
        // Phases sum exactly to total: no "Прочее" row, no redirect steps.
        assert!(w.try_find(("timing", 6usize)).is_none());
        assert!(w.try_find(("hop", 0usize)).is_none());
    });
}

#[gpui_kit::test]
fn timeline_shows_redirect_path_and_remainder(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    let hop_timings = Timings {
        dns: Duration::from_millis(5),
        connect: Duration::from_millis(10),
        tls: Duration::from_millis(15),
        ttfb: Duration::from_millis(15),
        download: Duration::from_millis(5),
        total: Duration::from_millis(50),
        ..Default::default()
    };
    let mut resp = response("{}", "application/json");
    resp.hops = vec![
        RedirectHop {
            method: Method::Get,
            url: "http://a.test/start".into(),
            status: 302,
            status_text: "Found".into(),
            location: "https://b.test/next".into(),
            headers: vec![("location".into(), "https://b.test/next".into())],
            remote_addr: None,
            timings: hop_timings.clone(),
        },
        RedirectHop {
            method: Method::Get,
            url: "https://b.test/next".into(),
            status: 301,
            status_text: "Moved Permanently".into(),
            location: "https://b.test/final".into(),
            headers: vec![],
            remote_addr: None,
            timings: hop_timings,
        },
    ];
    resp.redirect_history = vec![
        "http://a.test/start".into(),
        "https://b.test/next".into(),
        "https://b.test/final".into(),
    ];
    // Final hop: 148 ms of phases + 12 ms unaccounted ("Прочее").
    resp.timings.redirect = Duration::from_millis(100);
    resp.timings.total = Duration::from_millis(260);
    set(
        &view,
        window,
        ResponseState::Done {
            response: resp,
            schema_label: None,
            mismatches: vec![],
        },
    );
    window.update(|w, cx| {
        // The "↪ 2" pill opens the Timeline tab.
        w.click("response-redirects", cx);
        w.render_frame(cx);
        assert_eq!(label(w, "timeline-total"), "3 шага · Total 260 ms");
        assert_eq!(
            label(w, ("hop", 0usize)),
            "1 GET http://a.test/start → 302 Found → https://b.test/next"
        );
        assert_eq!(
            label(w, ("hop", 2usize)),
            "3 Ответ https://b.test/final → 201 Created"
        );
        assert!(w.try_find(("hop", 3usize)).is_none());
        for hop in 0..2usize {
            for phase in 0..5usize {
                let id = ("hop-bar", hop * 10 + phase);
                assert!(w.try_find(id).is_some(), "hop bar {hop}/{phase} missing");
            }
        }
        for i in [0usize, 1, 2, 3, 4, 6] {
            assert!(w.try_find(("timing-bar", i)).is_some(), "bar {i} missing");
        }
        // Steps come first, the final hop's detailed bars after them.
        let y = |w: &gpui_kit::Window, id: (&'static str, usize)| w.find(id).bounds().origin.y;
        assert!(y(w, ("hop", 1)) < y(w, ("hop", 2)));
        assert!(y(w, ("hop", 2)) < y(w, ("timing", 0)));
        assert!(y(w, ("timing", 4)) < y(w, ("timing", 6)));
    });
    assert_eq!(view.read_with(window, |v, _| v.tab()), ResponseTab::Timeline);
}

#[gpui_kit::test]
fn loading_cancel_by_button_and_escape(cx: &mut TestAppContext) {
    let (view, window, events) = open(cx);
    set(
        &view,
        window,
        ResponseState::Loading {
            host: "api.shop.dev".into(),
        },
    );
    window.update(|w, _| {
        for id in [
            "response-progress",
            "response-loading",
            "response-elapsed",
            "cancel",
        ] {
            assert!(w.try_find(id).is_some(), "{id} missing");
        }
    });
    // The elapsed ticker runs without panicking.
    window.executor().advance_clock(Duration::from_millis(350));
    window.update(|w, cx| {
        w.render_frame(cx);
        w.click("cancel", cx);
        let focus = view.read(cx).focus_handle(cx);
        w.focus(&focus, cx);
        w.press("escape", cx);
    });
    assert_eq!(
        *events.borrow(),
        vec![ResponseViewEvent::Cancel, ResponseViewEvent::Cancel]
    );

    // Esc does nothing once the response is in.
    set(&view, window, done(order_body(), vec![]));
    window.update(|w, cx| w.press("escape", cx));
    assert_eq!(events.borrow().len(), 2);
}

#[gpui_kit::test]
fn failed_state_retry(cx: &mut TestAppContext) {
    let (view, window, events) = open(cx);
    set(
        &view,
        window,
        ResponseState::Loading {
            host: "localhost:8080".into(),
        },
    );
    set(
        &view,
        window,
        ResponseState::Failed(SendError::Network {
            code: "ECONNREFUSED".into(),
            message: "connection refused".into(),
            host: "localhost:8080".into(),
        }),
    );
    window.update(|w, cx| {
        assert!(w.try_find("response-error").is_some());
        assert!(w.try_find("response-error-pill").is_some());
        assert!(w.try_find("response-progress").is_none());
        w.click(child("response-error", "secondary"), cx); // proxy settings: no-op
        w.click(child("response-error", "primary"), cx);
    });
    assert_eq!(*events.borrow(), vec![ResponseViewEvent::Retry]);

    for err in [
        SendError::Timeout { after_ms: 30_000 },
        SendError::InvalidUrl("htp:/x".into()),
        SendError::Cancelled,
    ] {
        set(&view, window, ResponseState::Failed(err));
        window.update(|w, _| assert!(w.try_find("response-error").is_some()));
    }
}

#[gpui_kit::test]
fn large_body_skips_pretty(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    let mut body = String::with_capacity(LARGE_BODY_BYTES + 64);
    body.push('[');
    while body.len() < LARGE_BODY_BYTES + 16 {
        body.push_str("{\"a\":1},");
    }
    body.push_str("{}]");
    set(&view, window, done(body.clone(), vec![]));
    window.update(|w, cx| {
        assert!(w.try_find("large-body").is_some());
        assert!(w.try_find("response-raw").is_some());
        w.within("body-mode").click(0usize, cx);
        w.render_frame(cx);
        assert!(
            w.try_find("response-raw").is_some(),
            "Pretty stays disabled"
        );
    });
    view.read_with(window, |v, _| {
        assert!(v.is_large());
        assert!(!v.pretty_available());
        assert_eq!(v.body_mode(), BodyMode::Raw);
    });
    // The whole body is in the (plain-text, unhighlighted) raw editor.
    assert_eq!(editor_text(&view, window, Body::Raw), body);
    assert_eq!(
        view.read_with(window, |v, cx| v.raw_editor().read(cx).language_name().to_string()),
        "text"
    );
}

#[gpui_kit::test]
fn browser_mode_renders_and_respects_suppression(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    let state = ResponseState::Done {
        response: response(
            "<html><head><style>h1{color:red}</style><script>document.title='js'</script></head><body><h1>Hi</h1></body></html>",
            "text/html; charset=utf-8",
        ),
        schema_label: None,
        mismatches: vec![],
    };
    view.update(window, |v, cx| {
        v.set_base_url(Some("https://shop.test/page".into()), cx)
    });
    set(&view, window, state);
    window.update(|w, cx| {
        w.within("body-mode").click(3usize, cx);
        w.render_frame(cx);
        assert!(w.try_find("response-browser").is_some());
    });
    assert_eq!(
        view.read_with(window, |v, _| v.body_mode()),
        BodyMode::Browser
    );

    // Covered by GPUI content (palette/popover): the native view is hidden.
    view.update(window, |v, cx| v.set_browser_suppressed(true, cx));
    window.update(|w, cx| w.render_frame(cx));
    assert!(!view.read_with(window, |v, _| v.browser_visible()));

    // Leaving Browser mode hides it too; a new response keeps the mode.
    view.update(window, |v, cx| v.set_browser_suppressed(false, cx));
    window.update(|w, cx| {
        w.within("body-mode").click(0usize, cx);
        w.render_frame(cx);
    });
    assert!(!view.read_with(window, |v, _| v.browser_visible()));
    window.update(|w, cx| {
        w.within("body-mode").click(3usize, cx);
        w.render_frame(cx);
    });
    set(
        &view,
        window,
        ResponseState::Done {
            response: response("{\"a\":1}", "application/json"),
            schema_label: None,
            mismatches: vec![],
        },
    );
    assert_eq!(
        view.read_with(window, |v, _| v.body_mode()),
        BodyMode::Browser
    );
}

// ---- scripts: Tests / Console ----------------------------------------------------

fn test_result(name: &str, error: Option<&str>) -> TestResult {
    TestResult {
        name: name.into(),
        passed: error.is_none(),
        error: error.map(Into::into),
    }
}

fn log(level: LogLevel, text: &str) -> LogLine {
    LogLine {
        level,
        text: text.into(),
    }
}

/// Post-response: 2 passed + 1 failed, two logs and a script error at line 3;
/// pre-request: one info log.
fn report() -> ScriptReport {
    ScriptReport {
        pre: Some(ScriptOutcome {
            logs: vec![log(LogLevel::Info, "pre start")],
            duration: Duration::from_millis(4),
            ..Default::default()
        }),
        post: Some(ScriptOutcome {
            tests: vec![
                test_result("status is 201", None),
                test_result("has id", None),
                test_result("total > 100", Some("expected 12.5 to be above 100")),
            ],
            logs: vec![
                log(LogLevel::Log, "searchId=42"),
                log(LogLevel::Warn, "slow"),
            ],
            error: Some(ScriptError {
                message: "x is not defined".into(),
                line: Some(3),
            }),
            duration: Duration::from_millis(8),
            ..Default::default()
        }),
    }
}

fn set_report(view: &Entity<ResponseView>, window: &mut VisualTestContext, r: Option<ScriptReport>) {
    window.update(|w, cx| {
        view.update(cx, |v, cx| v.set_script_report(r, cx));
        w.render_frame(cx);
    });
}

#[derive(Clone, Copy)]
enum Body {
    Pretty,
    Raw,
    Preview,
}

fn editor_text(view: &Entity<ResponseView>, window: &mut VisualTestContext, body: Body) -> String {
    view.read_with(window, |v, cx| {
        let editor = match body {
            Body::Pretty => v.pretty_editor(),
            Body::Raw => v.raw_editor(),
            Body::Preview => v.preview_editor(),
        };
        editor.read(cx).value().to_string()
    })
}

fn clipboard(window: &mut VisualTestContext) -> Option<String> {
    window.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()))
}

fn label(w: &gpui_kit::Window, id: impl Into<ElementId>) -> String {
    w.find(id).label().unwrap_or_default().to_string()
}

#[gpui_kit::test]
fn tests_tab_shows_counts_rows_and_failures(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    set(&view, window, done(order_body(), vec![]));
    set_report(&view, window, Some(report()));
    window.update(|w, cx| {
        assert_eq!(label(w, "tab-tests"), "Tests 2/3");
        assert_eq!(label(w, "tab-console"), "Console 4");
        w.click("tab-tests", cx);
        w.render_frame(cx);
        assert!(w.try_find("tests-list").is_some());
        assert_eq!(label(w, "tests-summary"), "2 passed · 1 failed · 12 ms");
        assert_eq!(label(w, ("test-row", 0usize)), "✓ status is 201");
        assert_eq!(label(w, ("test-row", 1usize)), "✓ has id");
        assert_eq!(
            label(w, ("test-row", 2usize)),
            "✗ total > 100: expected 12.5 to be above 100"
        );
        assert!(w.try_find(("test-row", 3usize)).is_none());
        assert!(w.try_find("tests-empty").is_none());
    });
    assert_eq!(view.read_with(window, |v, _| v.tab()), ResponseTab::Tests);
}

#[gpui_kit::test]
fn console_tab_lists_errors_then_logs(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    set(&view, window, done(order_body(), vec![]));
    set_report(&view, window, Some(report()));
    window.update(|w, cx| {
        w.click("tab-console", cx);
        w.render_frame(cx);
        assert!(w.try_find("console-list").is_some());
        assert_eq!(
            label(w, ("console-error", 0usize)),
            "Ошибка в Post-response скрипте, строка 3: x is not defined"
        );
        assert!(w.try_find(("console-error", 1usize)).is_none());
        assert_eq!(label(w, ("console-line", 0usize)), "pre pre start");
        assert_eq!(label(w, ("console-line", 1usize)), "post searchId=42");
        assert_eq!(label(w, ("console-line", 2usize)), "post slow");
        assert!(w.try_find(("console-line", 3usize)).is_none());
    });
}

#[gpui_kit::test]
fn script_error_pill_opens_console(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    set(&view, window, done(order_body(), vec![]));
    window.update(|w, _| assert!(w.try_find("script-error-pill").is_none()));
    set_report(&view, window, Some(report()));
    window.update(|w, cx| {
        w.click("script-error-pill", cx);
        w.render_frame(cx);
        assert!(w.try_find("console-list").is_some());
    });
    assert_eq!(view.read_with(window, |v, _| v.tab()), ResponseTab::Console);
}

#[gpui_kit::test]
fn script_report_survives_set_state(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    set_report(&view, window, Some(report()));
    set(&view, window, ResponseState::Loading { host: "x".into() });
    set(&view, window, done(order_body(), vec![]));
    assert_eq!(
        view.read_with(window, |v, _| v.script_report().cloned()),
        Some(report())
    );
    window.update(|w, _| assert_eq!(label(w, "tab-tests"), "Tests 2/3"));
    set_report(&view, window, None);
    assert!(view.read_with(window, |v, _| v.script_report().is_none()));
    window.update(|w, _| {
        assert_eq!(label(w, "tab-tests"), "Tests");
        assert_eq!(label(w, "tab-console"), "Console 0");
        assert!(w.try_find("script-error-pill").is_none());
    });
}

#[gpui_kit::test]
fn empty_report_shows_tests_empty(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    set(&view, window, done(order_body(), vec![]));
    set_report(&view, window, Some(ScriptReport::default()));
    window.update(|w, cx| {
        assert_eq!(label(w, "tab-tests"), "Tests");
        w.click("tab-tests", cx);
        w.render_frame(cx);
        assert!(w.try_find("tests-empty").is_some());
        assert!(w.try_find("tests-list").is_none());
    });
}

#[gpui_kit::test]
fn failed_pre_request_script_shows_error_in_console(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    let pre_failed = ScriptReport {
        pre: Some(ScriptOutcome {
            logs: vec![log(LogLevel::Log, "before")],
            error: Some(ScriptError {
                message: "boom".into(),
                line: Some(2),
            }),
            ..Default::default()
        }),
        post: None,
    };
    set(
        &view,
        window,
        ResponseState::Failed(SendError::Script {
            message: "boom".into(),
            line: Some(2),
        }),
    );
    set_report(&view, window, Some(pre_failed));
    window.update(|w, cx| {
        assert!(w.try_find("response-error").is_some());
        assert!(w.try_find("tab-error").is_some());
        assert_eq!(label(w, "tab-console"), "Console 2");
        w.click("tab-console", cx);
        w.render_frame(cx);
        assert_eq!(
            label(w, ("console-error", 0usize)),
            "Ошибка в Pre-request скрипте, строка 2: boom"
        );
        assert_eq!(label(w, ("console-line", 0usize)), "pre before");
        assert!(w.try_find("response-error").is_none());
        w.click("tab-error", cx);
        w.render_frame(cx);
        assert!(w.try_find("response-error").is_some());
    });
}

// ---- selection & copy -------------------------------------------------------------

#[gpui_kit::test]
fn pretty_body_is_read_only_and_copyable(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    set(&view, window, done(order_body(), vec![]));
    let pretty = pretty_json(order_body().as_bytes()).unwrap();
    window.update(|w, cx| {
        w.click("response-pretty", cx);
        w.input("xyz", cx);
        w.press("enter", cx);
        w.press("backspace", cx);
        w.press("cmd-v", cx);
    });
    assert_eq!(editor_text(&view, window, Body::Pretty), pretty, "typing is ignored");
    window.update(|w, cx| {
        w.press("cmd-a", cx);
        w.press("cmd-c", cx);
    });
    assert_eq!(clipboard(window).as_deref(), Some(pretty.as_str()));
}

#[gpui_kit::test]
fn raw_body_select_all_copies_original(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    set(&view, window, done(order_body(), vec![]));
    window.update(|w, cx| {
        w.within("body-mode").click(1usize, cx);
        w.click("response-raw", cx);
        w.press("cmd-a", cx);
        w.press("cmd-c", cx);
    });
    assert_eq!(clipboard(window), Some(order_body()));
}

#[gpui_kit::test]
fn console_text_is_selectable_and_copyable(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    set(&view, window, done(order_body(), vec![]));
    set_report(&view, window, Some(report()));
    window.update(|w, cx| {
        w.click("tab-console", cx);
        w.render_frame(cx);
        let from = w.find(("console-line", 0usize)).bounds();
        let to = w.find(("console-line", 2usize)).bounds();
        // From the start of the first message to past the end of the last one.
        let start = gpui_kit::point(from.origin.x + gpui_kit::px(50.), from.center().y);
        let end = gpui_kit::point(to.right() - gpui_kit::px(2.), to.center().y);
        w.drag(start, end, cx);
        w.press("cmd-c", cx);
    });
    let copied = clipboard(window).unwrap_or_default();
    for text in ["pre start", "searchId=42", "slow"] {
        assert!(copied.contains(text), "{text:?} not in {copied:?}");
    }
}

#[gpui_kit::test]
fn header_values_are_selectable(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    set(&view, window, done(order_body(), vec![]));
    window.update(|w, cx| {
        w.click("tab-headers", cx);
        w.render_frame(cx);
        let from = w.find(("header", 0usize)).bounds();
        let to = w.find(("header", 2usize)).bounds();
        let start = gpui_kit::point(from.origin.x + gpui_kit::px(16.), from.center().y);
        let end = gpui_kit::point(to.right() - gpui_kit::px(40.), to.center().y);
        w.drag(start, end, cx);
        w.press("cmd-c", cx);
    });
    let copied = clipboard(window).unwrap_or_default();
    for text in ["content-type", "x-request-id", "req-42", "no-store"] {
        assert!(copied.contains(text), "{text:?} not in {copied:?}");
    }
}
