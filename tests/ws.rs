//! WebSocket client (core::ws) against a local tokio-tungstenite echo server,
//! plus the WebSocket tab in the workspace.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use rivet::app::{self, Workspace};
use rivet::core::ws::{self, WsDir, WsEvent, WsEvents, WsKind};
use rivet::core::{
    KeyValue, Project, RequestKind, RequestSpec, ResolvedRequest, Settings, rt, store,
};
use rivet::theme::Palette;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

const WAIT: Duration = Duration::from_secs(5);

/// Handshake headers seen by the server, per connection.
type Seen = Arc<Mutex<Vec<Vec<(String, String)>>>>;

async fn echo<S>(stream: S, seen: Seen)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let callback = move |req: &Request, mut resp: Response| {
        let headers: Vec<(String, String)> = req
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        if let Some(proto) = req.headers().get("sec-websocket-protocol") {
            let first = proto.to_str().unwrap().split(',').next().unwrap().trim().to_string();
            resp.headers_mut()
                .insert("sec-websocket-protocol", first.parse().unwrap());
        }
        seen.lock().unwrap().push(headers);
        Ok(resp)
    };
    let Ok(mut socket) = tokio_tungstenite::accept_hdr_async(stream, callback).await else {
        return;
    };
    while let Some(Ok(msg)) = socket.next().await {
        match msg {
            Message::Text(t) if t.as_str() == "close-me" => {
                let _ = socket
                    .send(Message::Close(Some(CloseFrame {
                        code: CloseCode::from(4001),
                        reason: "bye".into(),
                    })))
                    .await;
            }
            Message::Text(_) | Message::Binary(_) => {
                if socket.send(msg).await.is_err() {
                    break;
                }
            }
            _ => {}
        }
    }
}

/// Plain `ws://` echo server; returns its URL and the recorded handshakes.
fn serve() -> (String, Seen) {
    let seen: Seen = Arc::default();
    let listener = rt::block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = listener.local_addr().unwrap();
    let s = seen.clone();
    rt::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            rt::spawn(echo(tcp, s.clone()));
        }
    });
    (format!("ws://{addr}/echo"), seen)
}

/// `wss://` echo server with a self-signed certificate.
fn serve_tls() -> String {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let der = cert.cert.der().clone();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(cert.signing_key.serialize_der().into());
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![der], key)
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = rt::block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = listener.local_addr().unwrap();
    rt::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            rt::spawn(async move {
                if let Ok(tls) = acceptor.accept(tcp).await {
                    echo(tls, Seen::default()).await;
                }
            });
        }
    });
    format!("wss://localhost:{}/secure", addr.port())
}

fn request(url: &str, headers: Vec<(&str, &str)>) -> ResolvedRequest {
    let mut spec = RequestSpec::untitled_ws();
    spec.url = url.into();
    spec.headers = headers
        .into_iter()
        .map(|(k, v)| KeyValue::new(k, v))
        .collect();
    ws::resolve(&spec, None).unwrap()
}

fn next(events: &mut WsEvents) -> WsEvent {
    events.recv_timeout(WAIT).expect("event expected")
}

/// Skips `Connecting` and returns the `Open` event.
fn wait_open(events: &mut WsEvents) -> WsEvent {
    assert_eq!(next(events), WsEvent::Connecting);
    let open = next(events);
    assert!(matches!(open, WsEvent::Open { .. }), "unexpected {open:?}");
    open
}

fn expect_message(events: &mut WsEvents, dir: WsDir, kind: WsKind, data: &[u8]) {
    match next(events) {
        WsEvent::Message(m) => {
            assert_eq!((m.dir, m.kind), (dir, kind));
            assert_eq!(m.data, data);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn text_binary_ping_and_close() {
    let (url, _) = serve();
    let (handle, mut events) = ws::connect(&request(&url, vec![]), &Settings::default());
    match wait_open(&mut events) {
        WsEvent::Open { status, protocol, .. } => {
            assert_eq!(status, 101);
            assert_eq!(protocol, None);
        }
        _ => unreachable!(),
    }

    assert!(handle.send_text("hello"));
    expect_message(&mut events, WsDir::Out, WsKind::Text, b"hello");
    expect_message(&mut events, WsDir::In, WsKind::Text, b"hello");

    handle.send_binary(vec![0u8, 1, 2, 255]);
    expect_message(&mut events, WsDir::Out, WsKind::Binary, &[0, 1, 2, 255]);
    expect_message(&mut events, WsDir::In, WsKind::Binary, &[0, 1, 2, 255]);

    handle.ping(b"p1".to_vec());
    expect_message(&mut events, WsDir::Out, WsKind::Ping, b"p1");
    expect_message(&mut events, WsDir::In, WsKind::Pong, b"p1");

    handle.close(1000, "done");
    assert_eq!(
        next(&mut events),
        WsEvent::Closed { code: 1000, reason: "done".into() }
    );
    assert_eq!(events.recv_timeout(WAIT), None);
    assert!(!handle.send_text("late"));
}

#[test]
fn server_initiated_close_reports_its_code() {
    let (url, _) = serve();
    let (handle, mut events) = ws::connect(&request(&url, vec![]), &Settings::default());
    wait_open(&mut events);
    handle.send_text("close-me");
    expect_message(&mut events, WsDir::Out, WsKind::Text, b"close-me");
    assert_eq!(
        next(&mut events),
        WsEvent::Closed { code: 4001, reason: "bye".into() }
    );
}

#[test]
fn handshake_headers_and_subprotocol_reach_the_server() {
    let (url, seen) = serve();
    let http_url = url.replace("ws://", "http://");
    let (_handle, mut events) = ws::connect(
        &request(
            &http_url,
            vec![("X-Custom", "42"), ("Sec-WebSocket-Protocol", "chat, superchat")],
        ),
        &Settings::default(),
    );
    match wait_open(&mut events) {
        WsEvent::Open { protocol, headers, .. } => {
            assert_eq!(protocol.as_deref(), Some("chat"));
            assert!(headers.iter().any(|(k, _)| k == "sec-websocket-accept"));
        }
        _ => unreachable!(),
    }
    let seen = seen.lock().unwrap();
    assert!(seen[0].iter().any(|(k, v)| k == "x-custom" && v == "42"));
}

#[test]
fn wss_with_self_signed_cert_needs_ssl_verify_off() {
    let url = serve_tls();
    let (_, mut events) = ws::connect(&request(&url, vec![]), &Settings::default());
    assert_eq!(next(&mut events), WsEvent::Connecting);
    match next(&mut events) {
        WsEvent::Error(e) => assert!(e.contains("TLS"), "{e}"),
        other => panic!("verification must fail, got {other:?}"),
    }

    let settings = Settings { ssl_verify: false, ..Settings::default() };
    let (handle, mut events) = ws::connect(&request(&url, vec![]), &settings);
    wait_open(&mut events);
    handle.send_text("secure");
    expect_message(&mut events, WsDir::Out, WsKind::Text, b"secure");
    expect_message(&mut events, WsDir::In, WsKind::Text, b"secure");
}

#[test]
fn refused_connection_is_an_error() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let (_, mut events) = ws::connect(
        &request(&format!("ws://127.0.0.1:{port}/"), vec![]),
        &Settings::default(),
    );
    assert_eq!(next(&mut events), WsEvent::Connecting);
    match next(&mut events) {
        WsEvent::Error(e) => assert!(e.starts_with("ECONNREFUSED"), "{e}"),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn old_project_without_kind_loads_as_http() {
    let json = r#"{
        "name": "Old",
        "environments": [],
        "collections": [{"id": "c", "name": "C", "requests": [
            {"id": "r", "name": "Get", "method": "GET", "url": "https://x.test"}
        ]}]
    }"#;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.json");
    std::fs::write(&path, json).unwrap();
    let project = store::load(&path).unwrap();
    let r = &project.collections[0].requests[0];
    assert_eq!(r.kind, RequestKind::Http);
    assert!(r.ws.messages.is_empty());

    // HTTP requests keep the old JSON shape; WS requests round-trip.
    let text = serde_json::to_string(r).unwrap();
    assert!(!text.contains("\"kind\":\"http\"") && !text.contains("\"ws\":"));
    let mut w = RequestSpec::untitled_ws();
    w.ws.messages.push(rivet::core::WsTemplate { name: "hi".into(), body: "{}".into() });
    let back: RequestSpec = serde_json::from_str(&serde_json::to_string(&w).unwrap()).unwrap();
    assert_eq!(back, w);
}

// ---- UI ------------------------------------------------------------------------

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

fn settle(cx: &mut VisualTestContext, done: impl Fn(&mut VisualTestContext) -> bool) {
    for _ in 0..250 {
        std::thread::sleep(Duration::from_millis(20));
        cx.executor().advance_clock(Duration::from_millis(20));
        cx.run_until_parked();
        if done(cx) {
            return;
        }
    }
    panic!("timed out waiting for the WebSocket UI");
}

#[gpui_kit::test]
fn websocket_tab_connects_sends_and_logs(cx: &mut TestAppContext) {
    let (url, _) = serve();
    let (view, window) = open_with(cx, store::seed_project());

    // "+" opens the new-tab menu; pick WebSocket.
    window.update(|w, cx| {
        w.click("new-tab-menu", cx);
        w.render_frame(cx);
        w.click("new-tab-ws", cx);
        w.render_frame(cx);
    });
    view.read_with(window, |v, _| {
        assert_eq!(v.tabs.len(), 2);
        assert!(v.active().unwrap().is_ws());
    });
    window.update(|w, cx| {
        assert!(w.try_find("ws-view").is_some());
        assert!(w.try_find("ws-scheme").is_some());
        assert!(w.try_find("method-select").is_none());
        w.click("url", cx);
        w.input(&url, cx);
    });
    // Input events reach the workspace once the update above has finished.
    window.update(|w, cx| {
        w.render_frame(cx);
        w.click("ws-connect", cx);
    });
    let ws_view = view.read_with(window, |v, _| v.ws_view.clone());
    settle(window, |cx| {
        ws_view.read_with(cx, |v, _| v.status() == app::ws_view::WsStatus::Open)
    });

    window.update(|w, cx| {
        w.render_frame(cx);
        w.click("ws-composer", cx);
        w.input("ping-from-ui", cx);
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        w.click("ws-send", cx);
    });
    settle(window, |cx| ws_view.read_with(cx, |v, _| v.log_len() >= 2));
    ws_view.read_with(window, |v, _| {
        let rows = v.log();
        assert_eq!(rows[0].dir, WsDir::Out);
        assert_eq!(rows[1].dir, WsDir::In);
        assert_eq!(rows[1].data, b"ping-from-ui");
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find(("ws-row", 0usize)).is_some());
        assert!(w.try_find(("ws-row", 1usize)).is_some());
        w.click(("ws-row", 1usize), cx);
        w.render_frame(cx);
        assert!(w.try_find("ws-details").is_some());
        w.click("ws-connect", cx); // Disconnect
    });
    settle(window, |cx| {
        ws_view.read_with(cx, |v, _| v.status() == app::ws_view::WsStatus::Closed)
    });
    view.read_with(window, |v, _| {
        let entry = &v.project.history[0];
        assert_eq!(entry.status, Some(101));
        assert!(entry.request.is_ws());
    });
}

#[gpui_kit::test]
fn palette_action_and_save_keep_kind(cx: &mut TestAppContext) {
    let mut project = store::seed_project();
    project.collections.push(rivet::core::Collection::new("Мои запросы"));
    let (view, window) = open_with(cx, project);
    view.update_in(window, |v, window, cx| v.new_ws_tab(window, cx));
    window.update(|w, cx| {
        w.click("url", cx);
        w.input("wss://echo.test/socket", cx);
        w.render_frame(cx);
    });
    window.update(|w, cx| {
        w.press("cmd-s", cx);
        w.render_frame(cx);
        w.click("dialog-confirm", cx);
        w.render_frame(cx);
    });
    view.read_with(window, |v, _| {
        let saved = &v.project.collections[0].requests[0];
        assert!(saved.is_ws());
        assert_eq!(saved.url, "wss://echo.test/socket");
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find(("saved-ws", 0usize)).is_some());
    });
}
