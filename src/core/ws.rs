//! WebSocket client (ws:// and wss://), UI-free.
//!
//! [`connect`] starts a connection on the shared Tokio runtime ([`super::rt`]) and
//! returns a [`WsHandle`] for sending plus a [`WsEvents`] stream of everything that
//! happens on the socket (handshake, both directions of traffic, close, errors).
//! The handshake reuses a resolved HTTP request: URL (http(s) is turned into
//! ws(s)), headers (incl. auth) and query params. `Sec-WebSocket-Protocol` set by
//! the user requests subprotocols; `settings.timeout_ms` bounds the handshake and
//! `settings.ssl_verify = false` accepts any server certificate.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

use super::rt;
use super::types::{Body, Environment, RequestSpec, ResolvedRequest, SendError, Settings};
use super::vars;

/// Close code sent by "Disconnect".
pub const NORMAL_CLOSURE: u16 = 1000;
/// No close frame was received (connection dropped).
pub const ABNORMAL_CLOSURE: u16 = 1006;
/// A close frame without a status code.
pub const NO_STATUS: u16 = 1005;
/// How long to wait for the server's close frame after sending ours.
const CLOSE_GRACE: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WsDir {
    /// Received from the server.
    In,
    /// Sent by us.
    Out,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WsKind {
    Text,
    Binary,
    Ping,
    Pong,
}

impl WsKind {
    pub fn label(self) -> &'static str {
        match self {
            WsKind::Text => "text",
            WsKind::Binary => "binary",
            WsKind::Ping => "ping",
            WsKind::Pong => "pong",
        }
    }
}

/// One frame of traffic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WsMessage {
    pub dir: WsDir,
    pub kind: WsKind,
    pub data: Vec<u8>,
    pub at: SystemTime,
}

impl WsMessage {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.data).into_owned()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WsEvent {
    Connecting,
    /// Handshake finished (`status` is 101).
    Open {
        status: u16,
        headers: Vec<(String, String)>,
        /// Subprotocol chosen by the server.
        protocol: Option<String>,
        /// Time from the start of the connection to the end of the handshake.
        elapsed: Duration,
    },
    Message(WsMessage),
    /// The connection is closed; no more events follow.
    Closed { code: u16, reason: String },
    /// A failure. Before `Open` it ends the stream; after `Open` a `Closed`
    /// (code 1006) follows.
    Error(String),
}

enum Command {
    Send(WsKind, Vec<u8>),
    Close(u16, String),
}

/// Sending side of a connection. Cheap to clone; dropping every clone closes
/// the connection normally.
#[derive(Clone, Debug)]
pub struct WsHandle {
    tx: mpsc::UnboundedSender<Command>,
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Command::Send(kind, data) => write!(f, "Send({kind:?}, {} bytes)", data.len()),
            Command::Close(code, reason) => write!(f, "Close({code}, {reason:?})"),
        }
    }
}

impl WsHandle {
    /// `false` once the connection task has ended.
    pub fn send_text(&self, text: impl Into<String>) -> bool {
        self.command(Command::Send(WsKind::Text, text.into().into_bytes()))
    }

    pub fn send_binary(&self, data: impl Into<Vec<u8>>) -> bool {
        self.command(Command::Send(WsKind::Binary, data.into()))
    }

    /// Sends a ping (payload is truncated to the 125 bytes the protocol allows).
    pub fn ping(&self, payload: impl Into<Vec<u8>>) -> bool {
        let mut data = payload.into();
        data.truncate(125);
        self.command(Command::Send(WsKind::Ping, data))
    }

    pub fn close(&self, code: u16, reason: impl Into<String>) -> bool {
        self.command(Command::Close(code, reason.into()))
    }

    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    fn command(&self, c: Command) -> bool {
        self.tx.send(c).is_ok()
    }
}

/// Event stream of a connection. `recv` is runtime-agnostic (works from the GPUI
/// executor too); `recv_timeout` blocks and is meant for sync callers and tests.
pub struct WsEvents {
    rx: mpsc::UnboundedReceiver<WsEvent>,
}

impl WsEvents {
    /// Next event, `None` once the connection is gone.
    pub async fn recv(&mut self) -> Option<WsEvent> {
        self.rx.recv().await
    }

    /// Non-blocking poll.
    pub fn try_recv(&mut self) -> Option<WsEvent> {
        self.rx.try_recv().ok()
    }

    /// `true` once the connection ended and every event was taken.
    pub fn is_finished(&self) -> bool {
        self.rx.is_closed() && self.rx.is_empty()
    }

    /// Blocking wait (never call from inside a Tokio task).
    pub fn recv_timeout(&mut self, timeout: Duration) -> Option<WsEvent> {
        rt::block_on(async { tokio::time::timeout(timeout, self.rx.recv()).await.ok().flatten() })
    }
}

/// `http(s)://` → `ws(s)://`; a missing scheme becomes `ws://`.
pub fn to_ws_url(url: &str) -> String {
    let trimmed = url.trim();
    match trimmed.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => format!("ws://{rest}"),
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("https") => format!("wss://{rest}"),
        Some(_) => trimmed.to_string(),
        None if trimmed.is_empty() => String::new(),
        None => format!("ws://{trimmed}"),
    }
}

/// `ws(s)://` → `http(s)://`, so URL handling shared with HTTP accepts it.
fn to_http_url(url: &str) -> String {
    let trimmed = url.trim();
    match trimmed.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("ws") => format!("http://{rest}"),
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("wss") => format!("https://{rest}"),
        _ => trimmed.to_string(),
    }
}

/// `"WSS"` for secure URLs (wss/https), `"WS"` otherwise.
pub fn scheme_label(url: &str) -> &'static str {
    let lower = url.trim_start().to_ascii_lowercase();
    if lower.starts_with("wss://") || lower.starts_with("https://") {
        "WSS"
    } else {
        "WS"
    }
}

/// [`vars::resolve`] for a WebSocket handshake: `{{vars}}`, params, headers and
/// auth are applied, the body is ignored and the URL ends up as ws(s)://.
pub fn resolve(req: &RequestSpec, env: Option<&Environment>) -> Result<ResolvedRequest, SendError> {
    let (url, mut missing) = vars::substitute(&req.url, env);
    let mut handshake = req.clone();
    handshake.url = to_http_url(&url);
    handshake.body = Body::default();
    let mut resolved = vars::resolve(&handshake, env).map_err(|e| match e {
        SendError::InvalidUrl(_) => SendError::InvalidUrl(req.url.clone()),
        other => other,
    })?;
    resolved.url = to_ws_url(&resolved.url);
    for m in std::mem::take(&mut resolved.missing_vars) {
        if !missing.contains(&m) {
            missing.push(m);
        }
    }
    resolved.missing_vars = missing;
    Ok(resolved)
}

/// Headers the WebSocket handshake sets itself.
fn reserved_header(name: &str) -> bool {
    [
        "host",
        "connection",
        "upgrade",
        "sec-websocket-key",
        "sec-websocket-version",
        "sec-websocket-extensions",
        "content-length",
    ]
    .iter()
    .any(|h| name.eq_ignore_ascii_case(h))
}

/// Opens a connection in the background. Events start with `Connecting`.
pub fn connect(resolved: &ResolvedRequest, settings: &Settings) -> (WsHandle, WsEvents) {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let url = to_ws_url(&resolved.url);
    let headers = resolved.headers.clone();
    let settings = settings.clone();
    rt::spawn(run(url, headers, settings, cmd_rx, ev_tx));
    (WsHandle { tx: cmd_tx }, WsEvents { rx: ev_rx })
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

type Socket = tokio_tungstenite::WebSocketStream<Box<dyn Io>>;
type Handshake = (Socket, tokio_tungstenite::tungstenite::handshake::client::Response);

async fn handshake(
    url: &str,
    headers: &[(String, String)],
    settings: &Settings,
) -> Result<Handshake, String> {
    let mut request = url
        .into_client_request()
        .map_err(|e| format!("Invalid URL: {e}"))?;
    for (k, v) in headers {
        if reserved_header(k) {
            continue;
        }
        let name = HeaderName::from_bytes(k.trim().as_bytes())
            .map_err(|_| format!("Invalid header name: {k}"))?;
        let value =
            HeaderValue::from_str(v).map_err(|_| format!("Invalid value of header {k}"))?;
        request.headers_mut().append(name, value);
    }
    let uri = request.uri().clone();
    let secure = uri.scheme_str() == Some("wss");
    let host = uri
        .host()
        .ok_or_else(|| format!("Invalid URL: {url}"))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });

    let tcp = TcpStream::connect((host.as_str(), port))
        .await
        .map_err(|e| network_error(&e, &host, port))?;
    let _ = tcp.set_nodelay(true);
    let stream: Box<dyn Io> = if secure {
        let connector = tokio_rustls::TlsConnector::from(Arc::new(tls_config(settings.ssl_verify)?));
        let name = rustls::pki_types::ServerName::try_from(host.clone())
            .map_err(|_| format!("Invalid TLS server name: {host}"))?;
        let tls = connector
            .connect(name, tcp)
            .await
            .map_err(|e| format!("TLS: {e}"))?;
        Box::new(tls)
    } else {
        Box::new(tcp)
    };
    tokio_tungstenite::client_async(request, stream)
        .await
        .map_err(|e| match e {
            WsError::Http(resp) => {
                let status = resp.status();
                let body = resp
                    .body()
                    .as_ref()
                    .map(|b| String::from_utf8_lossy(b).trim().to_string())
                    .filter(|b| !b.is_empty())
                    .map(|b| format!(": {}", b.chars().take(200).collect::<String>()))
                    .unwrap_or_default();
                format!(
                    "Handshake rejected: HTTP {} {}{body}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("")
                )
            }
            other => format!("Handshake failed: {other}"),
        })
}

fn network_error(e: &std::io::Error, host: &str, port: u16) -> String {
    use std::io::ErrorKind;
    let code = match e.kind() {
        ErrorKind::ConnectionRefused => "ECONNREFUSED",
        ErrorKind::ConnectionReset => "ECONNRESET",
        ErrorKind::TimedOut => "ETIMEDOUT",
        _ if e.to_string().contains("lookup") || e.to_string().contains("nodename") => "ENOTFOUND",
        _ => "ENETWORK",
    };
    format!("{code}: {host}:{port}: {e}")
}

fn tls_config(verify: bool) -> Result<rustls::ClientConfig, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS: {e}"))?;
    let config = if verify {
        let roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        builder.with_root_certificates(roots).with_no_client_auth()
    } else {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCert(provider)))
            .with_no_client_auth()
    };
    Ok(config)
}

/// `ssl_verify = false`: any certificate is accepted (signatures are still
/// checked so the handshake stays well-formed).
#[derive(Debug)]
struct AcceptAnyCert(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn message(dir: WsDir, kind: WsKind, data: Vec<u8>) -> WsEvent {
    WsEvent::Message(WsMessage {
        dir,
        kind,
        data,
        at: SystemTime::now(),
    })
}

async fn run(
    url: String,
    headers: Vec<(String, String)>,
    settings: Settings,
    mut commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<WsEvent>,
) {
    let emit = |e: WsEvent| {
        let _ = events.send(e);
    };
    emit(WsEvent::Connecting);
    let started = Instant::now();
    let timeout_ms = settings.timeout_ms.max(1);
    let (socket, response) = match tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        handshake(&url, &headers, &settings),
    )
    .await
    {
        Err(_) => return emit(WsEvent::Error(format!("Timed out after {timeout_ms} ms"))),
        Ok(Err(e)) => return emit(WsEvent::Error(e)),
        Ok(Ok(v)) => v,
    };
    let response_headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
        .collect();
    let protocol = response
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    emit(WsEvent::Open {
        status: response.status().as_u16(),
        headers: response_headers,
        protocol,
        elapsed: started.elapsed(),
    });

    let (mut sink, mut stream) = socket.split();
    // Our close frame, once sent; and the server's, once received.
    let mut sent_close: Option<(u16, String)> = None;
    let mut received_close: Option<(u16, String)> = None;
    let mut commands_open = true;
    let grace = tokio::time::sleep(Duration::from_secs(86_400 * 365));
    tokio::pin!(grace);

    loop {
        tokio::select! {
            cmd = commands.recv(), if commands_open && sent_close.is_none() && received_close.is_none() => {
                let (code, reason) = match cmd {
                    Some(Command::Send(kind, data)) => {
                        let msg = match kind {
                            WsKind::Text => match String::from_utf8(data.clone()) {
                                Ok(text) => Message::text(text),
                                Err(_) => Message::binary(data.clone()),
                            },
                            WsKind::Binary => Message::binary(data.clone()),
                            WsKind::Ping => Message::Ping(data.clone().into()),
                            WsKind::Pong => Message::Pong(data.clone().into()),
                        };
                        match sink.send(msg).await {
                            Ok(()) => emit(message(WsDir::Out, kind, data)),
                            Err(e) => {
                                emit(WsEvent::Error(format!("Send failed: {e}")));
                                emit(WsEvent::Closed { code: ABNORMAL_CLOSURE, reason: String::new() });
                                return;
                            }
                        }
                        continue;
                    }
                    Some(Command::Close(code, reason)) => (code, reason),
                    // Every handle dropped: close politely.
                    None => {
                        commands_open = false;
                        (NORMAL_CLOSURE, String::new())
                    }
                };
                let frame = CloseFrame { code: CloseCode::from(code), reason: reason.clone().into() };
                if sink.send(Message::Close(Some(frame))).await.is_err() {
                    emit(WsEvent::Closed { code, reason });
                    return;
                }
                sent_close = Some((code, reason));
                grace.as_mut().reset(tokio::time::Instant::now() + CLOSE_GRACE);
            }
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(t))) => emit(message(WsDir::In, WsKind::Text, t.as_bytes().to_vec())),
                Some(Ok(Message::Binary(b))) => emit(message(WsDir::In, WsKind::Binary, b.to_vec())),
                Some(Ok(Message::Ping(b))) => emit(message(WsDir::In, WsKind::Ping, b.to_vec())),
                Some(Ok(Message::Pong(b))) => emit(message(WsDir::In, WsKind::Pong, b.to_vec())),
                Some(Ok(Message::Close(frame))) => {
                    received_close = Some(match frame {
                        Some(f) => (u16::from(f.code), f.reason.to_string()),
                        None => (NO_STATUS, String::new()),
                    });
                    // The reply (for a server-initiated close) is flushed by the
                    // next read; the stream then ends.
                    if sent_close.is_none() {
                        grace.as_mut().reset(tokio::time::Instant::now() + CLOSE_GRACE);
                    }
                }
                Some(Ok(Message::Frame(_))) => {}
                Some(Err(WsError::ConnectionClosed | WsError::AlreadyClosed)) | None => {
                    let (code, reason) = received_close
                        .or(sent_close)
                        .unwrap_or((ABNORMAL_CLOSURE, String::new()));
                    emit(WsEvent::Closed { code, reason });
                    return;
                }
                Some(Err(e)) => {
                    if let Some((code, reason)) = received_close.or(sent_close) {
                        emit(WsEvent::Closed { code, reason });
                    } else {
                        emit(WsEvent::Error(e.to_string()));
                        emit(WsEvent::Closed { code: ABNORMAL_CLOSURE, reason: String::new() });
                    }
                    return;
                }
            },
            _ = &mut grace, if sent_close.is_some() || received_close.is_some() => {
                let (code, reason) = received_close
                    .or(sent_close)
                    .unwrap_or((ABNORMAL_CLOSURE, String::new()));
                emit(WsEvent::Closed { code, reason });
                return;
            }
        }
    }
}

/// `1.2 KB` style size (same rules as HTTP).
pub fn format_size(bytes: usize) -> String {
    super::http::format_size(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_schemes() {
        assert_eq!(to_ws_url("http://a/b"), "ws://a/b");
        assert_eq!(to_ws_url("HTTPS://a"), "wss://a");
        assert_eq!(to_ws_url("wss://a"), "wss://a");
        assert_eq!(to_ws_url("a:1/x"), "ws://a:1/x");
        assert_eq!(scheme_label("https://x"), "WSS");
        assert_eq!(scheme_label("ws://x"), "WS");
    }

    #[test]
    fn resolves_vars_params_and_auth() {
        let mut env = Environment::new("Dev", super::super::types::EnvColor::Green);
        env.vars.push(super::super::types::KeyValue::new("host", "wss://h.test"));
        let mut req = RequestSpec::untitled_ws();
        req.url = "{{host}}/chat".into();
        req.params.push(super::super::types::KeyValue::new("room", "1"));
        req.auth = super::super::types::Auth::Bearer { token: "t".into() };
        req.body = Body::json("{\"ignored\":true}");
        let r = resolve(&req, Some(&env)).unwrap();
        assert_eq!(r.url, "wss://h.test/chat?room=1");
        assert!(r.headers.iter().any(|(k, v)| k == "Authorization" && v == "Bearer t"));
        assert!(!r.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")));
        assert!(r.body.is_none());
        let missing = resolve(&RequestSpec { url: "ws://{{nope}}/x".into(), ..req }, None);
        assert!(missing.is_err());
    }
}
