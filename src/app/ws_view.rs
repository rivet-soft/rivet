//! WebSocket tab body: message composer, saved templates, handshake headers and
//! auth on the left; connection info and a virtualized message log with a
//! details pane on the right. The URL line and Connect button live in the
//! workspace request line (`request.rs`); connections are kept per request id so
//! switching tabs never drops a socket.

use std::collections::HashMap;
use std::ops::Range;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui_kit::component::input::{Editor, EditorState, Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::core::ws::{self, WsDir, WsEvent, WsHandle, WsKind, WsMessage};
use crate::core::{Auth, KeyValue, RequestSpec, ResolvedRequest, Settings, WsTemplate, http};
use crate::theme::{ActivePalette, MONO, Palette};
use crate::ui::{Button, IconButton, IconName, Pill, Segmented, StatusDot, icon, shortcut_hint};

const ROW_HEIGHT: f32 = 26.;
const PREVIEW_CHARS: usize = 160;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WsStatus {
    #[default]
    Idle,
    Connecting,
    Open,
    Closed,
    Failed,
}

impl WsStatus {
    pub fn is_live(self) -> bool {
        matches!(self, WsStatus::Connecting | WsStatus::Open)
    }
}

/// How the composer text is sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ComposeMode {
    #[default]
    Text,
    Json,
    /// Hex bytes (`de ad be ef`, `0xDEAD`), sent as a binary frame.
    Binary,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WsLeftTab {
    #[default]
    Message,
    Headers,
    Auth,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AuthKind {
    #[default]
    None,
    Inherit,
    Bearer,
    Basic,
    ApiKey,
}

impl AuthKind {
    const ALL: [AuthKind; 5] = [
        AuthKind::None,
        AuthKind::Inherit,
        AuthKind::Bearer,
        AuthKind::Basic,
        AuthKind::ApiKey,
    ];

    fn of(auth: &Auth) -> Self {
        match auth {
            Auth::None => AuthKind::None,
            Auth::Inherit => AuthKind::Inherit,
            Auth::Bearer { .. } => AuthKind::Bearer,
            Auth::Basic { .. } => AuthKind::Basic,
            Auth::ApiKey { .. } => AuthKind::ApiKey,
        }
    }

    fn label(self) -> &'static str {
        match self {
            AuthKind::None => "None",
            AuthKind::Inherit => "Inherit",
            AuthKind::Bearer => "Bearer",
            AuthKind::Basic => "Basic",
            AuthKind::ApiKey => "API Key",
        }
    }
}

pub enum WsViewEvent {
    /// Headers, auth or templates changed: copy them into the tab's request.
    Changed,
    /// A connection ended; record it in history.
    Finished {
        request: RequestSpec,
        /// 101 when the handshake succeeded.
        status: Option<u16>,
        /// How long the socket was open.
        duration_ms: Option<u64>,
    },
}

/// Handshake result shown in the info header.
#[derive(Clone, Debug)]
struct OpenInfo {
    status: u16,
    protocol: Option<String>,
    handshake: Duration,
    opened_at: Instant,
}

struct Session {
    generation: u64,
    request: RequestSpec,
    status: WsStatus,
    handle: Option<WsHandle>,
    log: Vec<WsMessage>,
    sent: usize,
    received: usize,
    info: Option<OpenInfo>,
    open_for: Option<Duration>,
    error: Option<String>,
    close: Option<(u16, String)>,
    reported: bool,
    _task: Option<Task<()>>,
    _ticker: Option<Task<()>>,
}

struct HeaderRow {
    enabled: bool,
    description: String,
    key: Entity<InputState>,
    value: Entity<InputState>,
    _subs: [Subscription; 2],
}

pub struct WsView {
    spec: RequestSpec,
    sessions: HashMap<String, Session>,
    generation: u64,
    left_tab: WsLeftTab,
    mode: ComposeMode,
    composer: Entity<EditorState>,
    compose_error: Option<String>,
    template_name: Entity<InputState>,
    headers: Vec<HeaderRow>,
    auth_kind: AuthKind,
    token: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    api_key: Entity<InputState>,
    api_value: Entity<InputState>,
    filter_input: Entity<InputState>,
    filter: String,
    /// Log indices matching the filter, rebuilt each render.
    filtered: Vec<usize>,
    selected: Option<usize>,
    details: Entity<EditorState>,
    auto_scroll: bool,
    scroll: UniformListScrollHandle,
    copied: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<WsViewEvent> for WsView {}

// ---- formatting helpers -------------------------------------------------------

/// `HH:MM:SS.mmm` (UTC).
pub fn clock(at: SystemTime) -> String {
    let d = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() % 86_400;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        secs / 3600,
        secs / 60 % 60,
        secs % 60,
        d.subsec_millis()
    )
}

fn hex_line(data: &[u8], limit: usize) -> String {
    let mut s: Vec<String> = data.iter().take(limit).map(|b| format!("{b:02x}")).collect();
    if data.len() > limit {
        s.push("…".into());
    }
    s.join(" ")
}

/// One-line preview of a frame for the log.
pub fn preview(m: &WsMessage) -> String {
    match m.kind {
        WsKind::Binary => hex_line(&m.data, 32),
        _ => {
            let text = String::from_utf8_lossy(&m.data);
            let mut out: String = text
                .chars()
                .take(PREVIEW_CHARS)
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            if text.chars().count() > PREVIEW_CHARS {
                out.push('…');
            }
            out
        }
    }
}

/// Full content for the details pane: pretty JSON, text or a hex dump.
pub fn details_text(m: &WsMessage) -> String {
    if m.kind == WsKind::Binary {
        return m
            .data
            .chunks(16)
            .enumerate()
            .map(|(i, chunk)| {
                let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
                let ascii: String = chunk
                    .iter()
                    .map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '.' })
                    .collect();
                format!("{:08x}  {:<47}  {ascii}", i * 16, hex.join(" "))
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    let text = String::from_utf8_lossy(&m.data);
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .filter(|v| v.is_object() || v.is_array())
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| text.into_owned())
}

/// Parses `de ad be ef`, `deadbeef`, `0xde,0xad`.
pub fn parse_hex(text: &str) -> Result<Vec<u8>, String> {
    let digits: String = text
        .split(|c: char| c.is_whitespace() || c == ',')
        .map(|t| t.trim_start_matches("0x").trim_start_matches("0X"))
        .collect();
    if digits.len() % 2 != 0 {
        return Err("Нечётное число hex-цифр".into());
    }
    (0..digits.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&digits[i..i + 2], 16)
                .map_err(|_| format!("Не hex: «{}»", &digits[i..i + 2]))
        })
        .collect()
}

fn badge_color(secure: bool, p: &Palette) -> Hsla {
    if secure { p.purple } else { p.teal }
}

/// `WS` / `WSS` label, the WebSocket counterpart of `MethodBadge`.
#[derive(IntoElement)]
pub struct WsBadge {
    secure: bool,
    width: Option<Pixels>,
}

impl WsBadge {
    pub fn new(secure: bool) -> Self {
        Self { secure, width: None }
    }

    pub fn for_url(url: &str) -> Self {
        Self::new(ws::scheme_label(url) == "WSS")
    }

    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(px(w));
        self
    }
}

impl RenderOnce for WsBadge {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = cx.palette();
        div()
            .flex_none()
            .when_some(self.width, |s, w| s.w(w))
            .font_family(MONO)
            .text_size(px(10.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(badge_color(self.secure, p))
            .child(if self.secure { "WSS" } else { "WS" })
    }
}

/// Sidebar row for a saved WebSocket request (same look as `TreeRow::request`).
pub(crate) fn ws_tree_row(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    url: &str,
    depth: usize,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let p = *cx.palette();
    div()
        .id(id.into())
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(26.))
        .mx(px(4.))
        .pl(px(28. + 16. * depth as f32))
        // Room for the sidebar's absolutely positioned "···" menu button.
        .pr(px(40.))
        .rounded(px(5.))
        .cursor_pointer()
        .when(selected, |s| s.bg(p.surface_hi))
        .hover(|s| s.bg(p.surface_hi))
        .on_click(on_click)
        .child(WsBadge::for_url(url).width(34.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .text_color(if selected { p.text } else { p.text_dim })
                .child(label.into()),
        )
}

// ---- view -----------------------------------------------------------------------

impl WsView {
    pub fn new(spec: RequestSpec, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("json")
                .soft_wrap(true)
                .placeholder("Сообщение…")
        });
        let details = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("json")
                .line_number(true)
                .soft_wrap(true)
        });
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let template_name = input("Название шаблона", window, cx);
        let token = input("{{token}}", window, cx);
        let username = input("Username", window, cx);
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Password")
                .masked(true)
        });
        let api_key = input("X-API-Key", window, cx);
        let api_value = input("Value", window, cx);
        let filter_input = input("Фильтр сообщений", window, cx);

        let mut subs = vec![cx.subscribe_in(
            &filter_input,
            window,
            |this, state, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    this.filter = state.read(cx).value().trim().to_lowercase();
                    cx.notify();
                }
            },
        )];
        for state in [&token, &username, &password, &api_key, &api_value] {
            subs.push(cx.subscribe_in(state, window, |this, _, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    this.commit_auth(cx);
                }
            }));
        }
        subs.push(cx.subscribe_in(&composer, window, |this, _, ev: &InputEvent, _, cx| {
            if let InputEvent::Change = ev {
                if this.compose_error.take().is_some() {
                    cx.notify();
                }
            }
        }));

        let mut view = Self {
            spec: RequestSpec::untitled_ws(),
            sessions: HashMap::new(),
            generation: 0,
            left_tab: WsLeftTab::Message,
            mode: ComposeMode::Text,
            composer,
            compose_error: None,
            template_name,
            headers: Vec::new(),
            auth_kind: AuthKind::None,
            token,
            username,
            password,
            api_key,
            api_value,
            filter_input,
            filter: String::new(),
            filtered: Vec::new(),
            selected: None,
            details,
            auto_scroll: true,
            scroll: UniformListScrollHandle::new(),
            copied: false,
            _subscriptions: subs,
        };
        view.load_spec(spec, true, window, cx);
        view
    }

    // ---- accessors -----------------------------------------------------------

    pub fn spec(&self) -> &RequestSpec {
        &self.spec
    }

    fn session(&self) -> Option<&Session> {
        self.sessions.get(&self.spec.id)
    }

    /// Connection state of the request shown now.
    pub fn status(&self) -> WsStatus {
        self.status_of(&self.spec.id)
    }

    pub fn status_of(&self, request_id: &str) -> WsStatus {
        self.sessions
            .get(request_id)
            .map(|s| s.status)
            .unwrap_or_default()
    }

    pub fn is_live(&self, request_id: &str) -> bool {
        self.status_of(request_id).is_live()
    }

    /// Full message log of the shown request (unfiltered, oldest first).
    pub fn log(&self) -> &[WsMessage] {
        self.session().map(|s| s.log.as_slice()).unwrap_or(&[])
    }

    pub fn log_len(&self) -> usize {
        self.log().len()
    }

    pub fn last_error(&self) -> Option<&str> {
        self.session().and_then(|s| s.error.as_deref())
    }

    pub fn compose_error(&self) -> Option<&str> {
        self.compose_error.as_deref()
    }

    pub fn mode(&self) -> ComposeMode {
        self.mode
    }

    pub fn left_tab(&self) -> WsLeftTab {
        self.left_tab
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn auto_scroll(&self) -> bool {
        self.auto_scroll
    }

    pub fn composer(&self) -> &Entity<EditorState> {
        &self.composer
    }

    // ---- spec ------------------------------------------------------------------

    /// Shows `spec` (the active WebSocket tab). Inputs are reloaded only when the
    /// request or its headers/auth changed.
    pub fn set_spec(&mut self, spec: RequestSpec, window: &mut Window, cx: &mut Context<Self>) {
        let switched = spec.id != self.spec.id;
        self.load_spec(spec, switched, window, cx);
        cx.notify();
    }

    fn load_spec(
        &mut self,
        spec: RequestSpec,
        switched: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let reload_headers = switched || spec.headers != self.spec.headers;
        let reload_auth = switched || spec.auth != self.spec.auth;
        if switched {
            self.selected = None;
            self.compose_error = None;
        }
        self.spec = spec;
        if reload_headers {
            self.headers.clear();
            for kv in self.spec.headers.clone() {
                self.push_header_row(kv, window, cx);
            }
        }
        if reload_auth {
            self.auth_kind = AuthKind::of(&self.spec.auth);
            let (token, user, pass, key, value) = match &self.spec.auth {
                Auth::Bearer { token } => (token.clone(), "", "", "", ""),
                Auth::Basic { username, password } => (
                    String::new(),
                    username.as_str(),
                    password.as_str(),
                    "",
                    "",
                ),
                Auth::ApiKey { key, value, .. } => {
                    (String::new(), "", "", key.as_str(), value.as_str())
                }
                _ => (String::new(), "", "", "", ""),
            };
            let (user, pass, key, value) =
                (user.to_string(), pass.to_string(), key.to_string(), value.to_string());
            for (state, text) in [
                (&self.token, token),
                (&self.username, user),
                (&self.password, pass),
                (&self.api_key, key),
                (&self.api_value, value),
            ] {
                state.update(cx, |s, cx| s.set_value(text, window, cx));
            }
        }
    }

    fn push_header_row(&mut self, kv: KeyValue, window: &mut Window, cx: &mut Context<Self>) {
        let key = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("Header");
            s.set_value(kv.key.clone(), window, cx);
            s
        });
        let value = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("Value");
            s.set_value(kv.value.clone(), window, cx);
            s
        });
        let on_change = |this: &mut Self, _: &Entity<InputState>, ev: &InputEvent, _: &mut Window, cx: &mut Context<Self>| {
            if let InputEvent::Change = ev {
                this.commit_headers(cx);
            }
        };
        let subs = [
            cx.subscribe_in(&key, window, on_change),
            cx.subscribe_in(&value, window, on_change),
        ];
        self.headers.push(HeaderRow {
            enabled: kv.enabled,
            description: kv.description,
            key,
            value,
            _subs: subs,
        });
    }

    fn commit_headers(&mut self, cx: &mut Context<Self>) {
        self.spec.headers = self
            .headers
            .iter()
            .map(|row| {
                KeyValue::new(
                    row.key.read(cx).value().to_string(),
                    row.value.read(cx).value().to_string(),
                )
                .enabled(row.enabled)
                .description(row.description.clone())
            })
            .collect();
        cx.emit(WsViewEvent::Changed);
        cx.notify();
    }

    pub fn add_header(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.push_header_row(KeyValue::new("", ""), window, cx);
        self.commit_headers(cx);
        if let Some(row) = self.headers.last() {
            row.key.update(cx, |s, cx| s.focus(window, cx));
        }
    }

    fn remove_header(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.headers.len() {
            self.headers.remove(ix);
            self.commit_headers(cx);
        }
    }

    fn toggle_header(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.headers.get_mut(ix) {
            row.enabled = !row.enabled;
            self.commit_headers(cx);
        }
    }

    fn set_auth_kind(&mut self, kind: AuthKind, cx: &mut Context<Self>) {
        self.auth_kind = kind;
        self.commit_auth(cx);
    }

    fn commit_auth(&mut self, cx: &mut Context<Self>) {
        let v = |s: &Entity<InputState>, cx: &Context<Self>| s.read(cx).value().to_string();
        self.spec.auth = match self.auth_kind {
            AuthKind::None => Auth::None,
            AuthKind::Inherit => Auth::Inherit,
            AuthKind::Bearer => Auth::Bearer { token: v(&self.token, cx) },
            AuthKind::Basic => Auth::Basic {
                username: v(&self.username, cx),
                password: v(&self.password, cx),
            },
            AuthKind::ApiKey => Auth::ApiKey {
                key: v(&self.api_key, cx),
                value: v(&self.api_value, cx),
                in_query: matches!(self.spec.auth, Auth::ApiKey { in_query: true, .. }),
            },
        };
        cx.emit(WsViewEvent::Changed);
        cx.notify();
    }

    pub fn set_left_tab(&mut self, tab: WsLeftTab, cx: &mut Context<Self>) {
        self.left_tab = tab;
        cx.notify();
    }

    pub fn set_mode(&mut self, mode: ComposeMode, cx: &mut Context<Self>) {
        self.mode = mode;
        self.compose_error = None;
        cx.notify();
    }

    // ---- templates ---------------------------------------------------------------

    pub fn save_template(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.composer.read(cx).value().to_string();
        if body.trim().is_empty() {
            self.compose_error = Some("Пустое сообщение нельзя сохранить".into());
            cx.notify();
            return;
        }
        let typed = self.template_name.read(cx).value().trim().to_string();
        let name = if typed.is_empty() {
            let first: String = body.trim().chars().take(24).collect();
            first.replace('\n', " ")
        } else {
            typed
        };
        match self.spec.ws.messages.iter_mut().find(|t| t.name == name) {
            Some(t) => t.body = body,
            None => self.spec.ws.messages.push(WsTemplate { name, body }),
        }
        self.template_name
            .update(cx, |s, cx| s.set_value("", window, cx));
        cx.emit(WsViewEvent::Changed);
        cx.notify();
    }

    pub fn load_template(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = self.spec.ws.messages.get(ix) {
            let body = t.body.clone();
            self.composer
                .update(cx, |s, cx| s.set_value(body, window, cx));
            self.compose_error = None;
            cx.notify();
        }
    }

    pub fn delete_template(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.spec.ws.messages.len() {
            self.spec.ws.messages.remove(ix);
            cx.emit(WsViewEvent::Changed);
            cx.notify();
        }
    }

    // ---- connection ----------------------------------------------------------------

    /// Opens a connection for `request` (replacing a previous one of the same
    /// request). Events are applied on the GPUI executor as they arrive.
    pub fn connect(
        &mut self,
        request: RequestSpec,
        resolved: &ResolvedRequest,
        settings: &Settings,
        cx: &mut Context<Self>,
    ) {
        let id = request.id.clone();
        if let Some(old) = self.sessions.get(&id).and_then(|s| s.handle.clone()) {
            old.close(ws::NORMAL_CLOSURE, "");
        }
        self.generation += 1;
        let generation = self.generation;
        let (handle, mut events) = ws::connect(resolved, settings);
        let task_id = id.clone();
        // The deterministic test scheduler forbids wake-ups from the network
        // thread, so tests poll on (simulated) timers instead of awaiting.
        let polling = cx.background_executor().is_test();
        let task = cx.spawn(async move |this, cx| {
            loop {
                let ev = if polling {
                    match events.try_recv() {
                        Some(ev) => ev,
                        None if events.is_finished() => break,
                        None => {
                            cx.background_executor().timer(Duration::from_millis(10)).await;
                            continue;
                        }
                    }
                } else {
                    match events.recv().await {
                        Some(ev) => ev,
                        None => break,
                    }
                };
                let alive = this
                    .update(cx, |v, cx| v.apply(&task_id, generation, ev, cx))
                    .is_ok();
                if !alive {
                    return;
                }
            }
            this.update(cx, |v, cx| v.finish(&task_id, generation, cx)).ok();
        });
        // Keep the old log so a reconnect continues the conversation.
        let log = self.sessions.remove(&id).map(|s| s.log).unwrap_or_default();
        if id == self.spec.id {
            self.selected = self.selected.filter(|&i| i < log.len());
        }
        self.sessions.insert(
            id,
            Session {
                generation,
                request,
                status: WsStatus::Connecting,
                handle: Some(handle),
                log,
                sent: 0,
                received: 0,
                info: None,
                open_for: None,
                error: None,
                close: None,
                reported: false,
                _task: Some(task),
                _ticker: None,
            },
        );
        cx.notify();
    }

    /// Sends a normal close (1000) for the shown request.
    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        let id = self.spec.id.clone();
        self.disconnect_id(&id, cx);
    }

    pub fn disconnect_id(&mut self, request_id: &str, cx: &mut Context<Self>) {
        let Some(session) = self.sessions.get_mut(request_id) else {
            return;
        };
        match (&session.handle, session.status) {
            (Some(handle), WsStatus::Open) => {
                handle.close(ws::NORMAL_CLOSURE, "");
            }
            // Still handshaking: abandon it.
            (_, WsStatus::Connecting) => {
                session.handle = None;
                session._task = None;
                session.status = WsStatus::Closed;
                session.error = Some("Отменено".into());
                self.report(request_id, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    /// Forgets the session of a closed tab (closing its socket).
    pub fn drop_session(&mut self, request_id: &str, cx: &mut Context<Self>) {
        if let Some(handle) = self.sessions.get(request_id).and_then(|s| s.handle.clone()) {
            handle.close(ws::NORMAL_CLOSURE, "");
        }
        self.report(request_id, cx);
        self.sessions.remove(request_id);
        cx.notify();
    }

    fn apply(&mut self, id: &str, generation: u64, ev: WsEvent, cx: &mut Context<Self>) {
        let current = id == self.spec.id;
        let Some(s) = self.sessions.get_mut(id).filter(|s| s.generation == generation) else {
            return;
        };
        match ev {
            WsEvent::Connecting => s.status = WsStatus::Connecting,
            WsEvent::Open { status, protocol, elapsed, .. } => {
                s.status = WsStatus::Open;
                s.info = Some(OpenInfo {
                    status,
                    protocol,
                    handshake: elapsed,
                    opened_at: Instant::now(),
                });
                // Repaint the live duration once a second while open.
                s._ticker = Some(cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor().timer(Duration::from_secs(1)).await;
                        if this.update(cx, |_, cx| cx.notify()).is_err() {
                            return;
                        }
                    }
                }));
            }
            WsEvent::Message(m) => {
                match m.dir {
                    WsDir::In => s.received += 1,
                    WsDir::Out => s.sent += 1,
                }
                s.log.push(m);
                if current && self.auto_scroll {
                    self.scroll.scroll_to_bottom();
                }
            }
            WsEvent::Closed { code, reason } => {
                s.status = WsStatus::Closed;
                s.close = Some((code, reason));
                s.handle = None;
                s._ticker = None;
                s.open_for = s.info.as_ref().map(|i| i.opened_at.elapsed());
            }
            WsEvent::Error(e) => {
                if s.status == WsStatus::Connecting {
                    s.status = WsStatus::Failed;
                }
                s.error = Some(e);
            }
        }
        cx.notify();
    }

    fn finish(&mut self, id: &str, generation: u64, cx: &mut Context<Self>) {
        let Some(s) = self.sessions.get_mut(id).filter(|s| s.generation == generation) else {
            return;
        };
        if s.status.is_live() {
            s.status = if s.info.is_some() { WsStatus::Closed } else { WsStatus::Failed };
        }
        s.handle = None;
        s._ticker = None;
        if s.open_for.is_none() {
            s.open_for = s.info.as_ref().map(|i| i.opened_at.elapsed());
        }
        self.report(id, cx);
        cx.notify();
    }

    /// Emits `Finished` once per connection.
    fn report(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(s) = self.sessions.get_mut(id) else {
            return;
        };
        if s.reported {
            return;
        }
        s.reported = true;
        let duration = s
            .open_for
            .or_else(|| s.info.as_ref().map(|i| i.opened_at.elapsed()));
        cx.emit(WsViewEvent::Finished {
            request: s.request.clone(),
            status: s.info.as_ref().map(|i| i.status),
            duration_ms: duration.map(|d| d.as_millis() as u64),
        });
    }

    /// Sends the composer content in the current mode.
    pub fn send_message(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = self
            .session()
            .filter(|s| s.status == WsStatus::Open)
            .and_then(|s| s.handle.clone())
        else {
            self.compose_error = Some("Нет соединения — нажмите Connect".into());
            cx.notify();
            return;
        };
        let text = self.composer.read(cx).value().to_string();
        let result = match self.mode {
            ComposeMode::Text => Ok(handle.send_text(text)),
            ComposeMode::Json => serde_json::from_str::<serde_json::Value>(&text)
                .map_err(|e| format!("Невалидный JSON: {e}"))
                .map(|_| handle.send_text(text)),
            ComposeMode::Binary => parse_hex(&text).map(|bytes| handle.send_binary(bytes)),
        };
        self.compose_error = match result {
            Ok(true) => None,
            Ok(false) => Some("Соединение закрыто".into()),
            Err(e) => Some(e),
        };
        cx.notify();
    }

    pub fn ping(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self
            .session()
            .filter(|s| s.status == WsStatus::Open)
            .and_then(|s| s.handle.clone())
        {
            handle.ping(Vec::new());
        }
        cx.notify();
    }

    pub fn clear_log(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.spec.id.clone();
        if let Some(s) = self.sessions.get_mut(&id) {
            s.log.clear();
        }
        self.selected = None;
        self.details.update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }

    pub fn toggle_auto_scroll(&mut self, cx: &mut Context<Self>) {
        self.auto_scroll = !self.auto_scroll;
        if self.auto_scroll {
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    pub fn select(&mut self, ix: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = ix.filter(|&i| i < self.log_len());
        self.copied = false;
        let text = self
            .selected
            .and_then(|i| self.log().get(i))
            .map(details_text)
            .unwrap_or_default();
        self.details.update(cx, |s, cx| s.set_value(text, window, cx));
        cx.notify();
    }

    fn copy_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(m) = self.selected.and_then(|i| self.log().get(i)) {
            cx.write_to_clipboard(ClipboardItem::new_string(details_text(m)));
            self.copied = true;
            cx.notify();
        }
    }

    // ---- rendering -------------------------------------------------------------------

    fn render_info(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = *cx.palette();
        let session = self.session();
        let status = self.status();
        let (color, label): (Hsla, String) = match status {
            WsStatus::Idle => (p.text_dim, "Не подключено".into()),
            WsStatus::Connecting => (p.amber, "Подключение…".into()),
            WsStatus::Open => (p.online, "Подключено".into()),
            WsStatus::Closed => (
                p.text_dim,
                match session.and_then(|s| s.close.as_ref()) {
                    Some((code, reason)) if reason.is_empty() => format!("Закрыто · {code}"),
                    Some((code, reason)) => format!("Закрыто · {code} {reason}"),
                    None => "Закрыто".into(),
                },
            ),
            WsStatus::Failed => (p.danger, "Ошибка".into()),
        };
        let info = session.and_then(|s| s.info.clone());
        let duration = session.and_then(|s| {
            s.open_for
                .or_else(|| s.info.as_ref().map(|i| i.opened_at.elapsed()))
        });
        let (sent, received) = session.map(|s| (s.sent, s.received)).unwrap_or((0, 0));
        let error = session.and_then(|s| s.error.clone());
        div()
            .id("ws-info")
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap(px(10.))
            .px(px(14.))
            .py(px(7.))
            .border_b_1()
            .border_color(p.border)
            .text_size(px(11.5))
            .child(
                div()
                    .id("ws-status")
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(StatusDot::new(color))
                    .child(div().text_color(p.text).child(label))
                    .test_support(),
            )
            .when_some(info, |d, info| {
                d.child(
                    Pill::new(format!("{} Switching Protocols", info.status))
                        .color(p.online)
                        .mono(),
                )
                .when_some(info.protocol, |d, proto| {
                    d.child(Pill::new(format!("protocol: {proto}")).mono())
                })
                .child(
                    div()
                        .text_color(p.text_dim)
                        .child(format!("handshake {}", http::format_duration(info.handshake))),
                )
            })
            .when_some(duration, |d, dur| {
                d.child(
                    div()
                        .id("ws-duration")
                        .text_color(p.text_dim)
                        .child(format!("открыто {}", http::format_duration(dur)))
                        .test_support(),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .id("ws-counts")
                    .flex()
                    .gap(px(8.))
                    .font_family(MONO)
                    .text_size(px(11.))
                    .child(div().text_color(p.accent).child(format!("↑ {sent}")))
                    .child(div().text_color(p.online).child(format!("↓ {received}")))
                    .test_support(),
            )
            .when_some(error, |d, e| {
                d.child(
                    div()
                        .id("ws-error")
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_color(p.danger)
                        .child(icon(IconName::TriangleAlert, 12., p.danger))
                        .child(e)
                        .test_support(),
                )
            })
    }

    fn render_composer(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *cx.palette();
        let view = cx.entity().downgrade();
        let open = self.status() == WsStatus::Open;
        let mode_ix = self.mode as usize;
        let toolbar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(8.))
            .child(
                Segmented::new("ws-mode")
                    .items(["Text", "JSON", "Binary (hex)"])
                    .selected(mode_ix)
                    .on_select({
                        let view = view.clone();
                        move |ix, _, cx| {
                            let mode = [ComposeMode::Text, ComposeMode::Json, ComposeMode::Binary][ix];
                            view.update(cx, |v, cx| v.set_mode(mode, cx)).ok();
                        }
                    }),
            )
            .child(div().flex_1())
            .child(
                Button::new("ws-ping", "Ping")
                    .ghost()
                    .disabled(!open)
                    .on_click(cx.listener(|this, _, _, cx| this.ping(cx))),
            )
            .child(
                Button::new("ws-send", "Send")
                    .primary()
                    .disabled(!open)
                    .icon(IconName::SendHorizontal)
                    .kbd(shortcut_hint("⌘↵", "Ctrl+Enter"))
                    .on_click(cx.listener(|this, _, _, cx| this.send_message(cx))),
            );

        let editor = div()
            .id("ws-composer")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(80.))
            .border_t_1()
            .border_color(p.border)
            .child(
                Editor::new(&self.composer)
                    .appearance(false)
                    .bordered(false)
                    .font_family(MONO)
                    .text_size(px(12.))
                    .h(relative(1.)),
            )
            .test_support();

        let templates = self.spec.ws.messages.iter().enumerate().map(|(ix, t)| {
            div()
                .id(("ws-template", ix))
                .group("ws-template")
                .flex()
                .items_center()
                .gap(px(6.))
                .h(px(26.))
                .px(px(8.))
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|s| s.bg(p.surface_hi))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.load_template(ix, window, cx)
                }))
                .child(icon(IconName::Bookmark, 12., p.text_dim))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(t.name.clone()),
                )
                .child(
                    div()
                        .invisible()
                        .group_hover("ws-template", |s| s.visible())
                        .child(
                            IconButton::new(("ws-template-delete", ix), IconName::X)
                                .size(12.)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.delete_template(ix, cx)
                                })),
                        ),
                )
                .test_support()
        });

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(toolbar)
            .when_some(self.compose_error.clone(), |d, e| {
                d.child(
                    div()
                        .id("ws-compose-error")
                        .px(px(12.))
                        .pb(px(6.))
                        .text_size(px(11.))
                        .text_color(p.danger)
                        .child(e)
                        .test_support(),
                )
            })
            .child(editor)
            .child(
                div()
                    .id("ws-templates")
                    .flex()
                    .flex_col()
                    .flex_none()
                    .max_h(px(220.))
                    .overflow_y_scroll()
                    .gap(px(2.))
                    .p(px(8.))
                    .border_t_1()
                    .border_color(p.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(p.text_dim)
                                    .child("ШАБЛОНЫ"),
                            )
                            .child(
                                div().flex_1().child(
                                    Input::new(&self.template_name)
                                        .id("ws-template-name")
                                        .h(px(26.)),
                                ),
                            )
                            .child(
                                Button::new("ws-template-save", "Сохранить")
                                    .ghost()
                                    .icon(IconName::Plus)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save_template(window, cx)
                                    })),
                            ),
                    )
                    .when(self.spec.ws.messages.is_empty(), |d| {
                        d.child(
                            div()
                                .px(px(8.))
                                .py(px(4.))
                                .text_size(px(11.))
                                .text_color(p.text_dim)
                                .child("Сохраните сообщение, чтобы быстро отправлять его снова"),
                        )
                    })
                    .children(templates)
                    .test_support(),
            )
            .into_any_element()
    }

    fn render_headers(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *cx.palette();
        let rows = self.headers.iter().enumerate().map(|(ix, row)| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .id(("ws-header-toggle", ix))
                        .size(px(14.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(p.border)
                        .cursor_pointer()
                        .when(row.enabled, |d| {
                            d.bg(p.accent).child(icon(IconName::Check, 10., p.bg))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_header(ix, cx)))
                        .test_support(),
                )
                .child(
                    div()
                        .flex_1()
                        .child(Input::new(&row.key).id(("ws-header-key", ix)).h(px(28.))),
                )
                .child(
                    div().flex_1().child(
                        Input::new(&row.value)
                            .id(("ws-header-value", ix))
                            .font_family(MONO)
                            .h(px(28.)),
                    ),
                )
                .child(
                    IconButton::new(("ws-header-remove", ix), IconName::X)
                        .size(12.)
                        .on_click(cx.listener(move |this, _, _, cx| this.remove_header(ix, cx))),
                )
        });
        div()
            .id("ws-headers")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .gap(px(6.))
            .p(px(12.))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(p.text_dim)
                    .child("Заголовки рукопожатия. Sec-WebSocket-Protocol задаёт подпротоколы."),
            )
            .children(rows)
            .child(
                div().child(
                    Button::new("ws-header-add", "Добавить заголовок")
                        .ghost()
                        .icon(IconName::Plus)
                        .on_click(cx.listener(|this, _, window, cx| this.add_header(window, cx))),
                ),
            )
            .test_support()
            .into_any_element()
    }

    fn render_auth(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *cx.palette();
        let view = cx.entity().downgrade();
        let selected = AuthKind::ALL
            .iter()
            .position(|k| *k == self.auth_kind)
            .unwrap_or(0);
        let field = |label: &'static str, input: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(div().text_size(px(11.)).text_color(p.text_dim).child(label))
                .child(input)
        };
        let input = |state: &Entity<InputState>, id: &'static str| {
            Input::new(state).id(id).font_family(MONO).h(px(28.)).into_any_element()
        };
        let body = match self.auth_kind {
            AuthKind::None => div()
                .text_size(px(11.5))
                .text_color(p.text_dim)
                .child("Без авторизации")
                .into_any_element(),
            AuthKind::Inherit => div()
                .text_size(px(11.5))
                .text_color(p.text_dim)
                .child("Bearer-токен из переменной окружения {{token}}")
                .into_any_element(),
            AuthKind::Bearer => field("Token", input(&self.token, "ws-auth-token")).into_any_element(),
            AuthKind::Basic => div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(field("Username", input(&self.username, "ws-auth-username")))
                .child(field("Password", input(&self.password, "ws-auth-password")))
                .into_any_element(),
            AuthKind::ApiKey => div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(field("Key", input(&self.api_key, "ws-auth-key")))
                .child(field("Value", input(&self.api_value, "ws-auth-value")))
                .into_any_element(),
        };
        div()
            .id("ws-auth")
            .flex()
            .flex_col()
            .flex_1()
            .gap(px(10.))
            .p(px(12.))
            .child(
                Segmented::new("ws-auth-kind")
                    .items(AuthKind::ALL.map(AuthKind::label))
                    .selected(selected)
                    .on_select(move |ix, _, cx| {
                        view.update(cx, |v, cx| v.set_auth_kind(AuthKind::ALL[ix], cx))
                            .ok();
                    }),
            )
            .child(body)
            .test_support()
            .into_any_element()
    }

    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = *cx.palette();
        let Some(m) = self.log().get(ix) else {
            return div().h(px(ROW_HEIGHT)).into_any_element();
        };
        let (arrow, color) = match m.dir {
            WsDir::Out => (IconName::ArrowUp, p.accent),
            WsDir::In => (IconName::ArrowDown, p.online),
        };
        let selected = self.selected == Some(ix);
        div()
            .id(("ws-row", ix))
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(ROW_HEIGHT))
            .px(px(12.))
            .border_b_1()
            .border_color(p.border.opacity(0.5))
            .cursor_pointer()
            .when(selected, |d| d.bg(p.surface_hi))
            .hover(|d| d.bg(p.surface_hi))
            .on_click(cx.listener(move |this, _, window, cx| {
                let next = (this.selected != Some(ix)).then_some(ix);
                this.select(next, window, cx)
            }))
            .child(icon(arrow, 12., color))
            .child(
                div()
                    .flex_none()
                    .font_family(MONO)
                    .text_size(px(10.5))
                    .text_color(p.text_dim)
                    .child(clock(m.at)),
            )
            .when(m.kind != WsKind::Text, |d| {
                d.child(Pill::new(m.kind.label()).color(color).mono())
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .font_family(MONO)
                    .text_size(px(11.5))
                    .child(preview(m)),
            )
            .child(
                div()
                    .flex_none()
                    .font_family(MONO)
                    .text_size(px(10.5))
                    .text_color(p.text_dim)
                    .child(http::format_size(m.data.len())),
            )
            .test_support()
            .into_any_element()
    }

    fn render_log(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *cx.palette();
        let count = self.filtered.len();
        let toolbar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(8.))
            .border_b_1()
            .border_color(p.border)
            .child(
                div().flex_1().child(
                    Input::new(&self.filter_input)
                        .id("ws-filter")
                        .prefix(icon(IconName::Search, 13., p.text_dim))
                        .cleanable(true)
                        .h(px(28.)),
                ),
            )
            .child(
                Button::new("ws-autoscroll", "Автопрокрутка")
                    .ghost()
                    .when(self.auto_scroll, |b| b.icon(IconName::Check))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_auto_scroll(cx))),
            )
            .child(
                Button::new("ws-clear", "Очистить")
                    .ghost()
                    .on_click(cx.listener(|this, _, window, cx| this.clear_log(window, cx))),
            );

        let list = if count == 0 {
            div()
                .id("ws-log-empty")
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(p.text_dim)
                .child(if self.log_len() == 0 {
                    "Сообщений пока нет"
                } else {
                    "Ничего не найдено"
                })
                .test_support()
                .into_any_element()
        } else {
            div()
                .id("ws-log")
                .flex_1()
                .min_h_0()
                .child(
                    uniform_list(
                        "ws-log-list",
                        count,
                        cx.processor(|this, range: Range<usize>, _, cx| {
                            range
                                .filter_map(|i| this.filtered.get(i).copied())
                                .map(|ix| this.render_row(ix, cx))
                                .collect()
                        }),
                    )
                    .track_scroll(&self.scroll)
                    .size_full(),
                )
                .test_support()
                .into_any_element()
        };

        let details = self.selected.and_then(|i| self.log().get(i)).map(|m| {
            let color = if m.dir == WsDir::Out { p.accent } else { p.online };
            div()
                .id("ws-details")
                .flex()
                .flex_col()
                .flex_none()
                .h(relative(0.45))
                .border_t_1()
                .border_color(p.border)
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(8.))
                        .px(px(12.))
                        .py(px(6.))
                        .text_size(px(11.))
                        .child(
                            div()
                                .text_color(color)
                                .child(if m.dir == WsDir::Out { "↑ Отправлено" } else { "↓ Получено" }),
                        )
                        .child(Pill::new(m.kind.label()).mono())
                        .child(div().font_family(MONO).text_color(p.text_dim).child(clock(m.at)))
                        .child(
                            div()
                                .font_family(MONO)
                                .text_color(p.text_dim)
                                .child(http::format_size(m.data.len())),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("ws-details-copy", if self.copied { "Скопировано" } else { "Копировать" })
                                .ghost()
                                .icon(if self.copied { IconName::Check } else { IconName::Copy })
                                .on_click(cx.listener(|this, _, _, cx| this.copy_selected(cx))),
                        )
                        .child(
                            IconButton::new("ws-details-close", IconName::X)
                                .size(12.)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.select(None, window, cx)
                                })),
                        ),
                )
                .child(
                    div().flex_1().min_h_0().border_t_1().border_color(p.border).child(
                        Editor::new(&self.details)
                            .readonly(true)
                            .appearance(false)
                            .bordered(false)
                            .font_family(MONO)
                            .text_size(px(12.))
                            .h(relative(1.)),
                    ),
                )
                .test_support()
        });

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(toolbar)
            .child(list)
            .children(details)
            .into_any_element()
    }
}

impl Render for WsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *cx.palette();
        let filter = self.filter.clone();
        self.filtered = self
            .log()
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                filter.is_empty()
                    || m.kind.label() == filter
                    || preview(m).to_lowercase().contains(&filter)
                    || (m.data.len() > PREVIEW_CHARS
                        && String::from_utf8_lossy(&m.data).to_lowercase().contains(&filter))
            })
            .map(|(i, _)| i)
            .collect();

        let view = cx.entity().downgrade();
        let headers_count = self.spec.headers.iter().filter(|h| h.enabled && !h.key.is_empty()).count();
        let tabs = Segmented::new("ws-left-tabs")
            .items([
                "Сообщение".to_string(),
                if headers_count > 0 {
                    format!("Headers ({headers_count})")
                } else {
                    "Headers".to_string()
                },
                "Auth".to_string(),
            ])
            .selected(self.left_tab as usize)
            .on_select(move |ix, _, cx| {
                let tab = [WsLeftTab::Message, WsLeftTab::Headers, WsLeftTab::Auth][ix];
                view.update(cx, |v, cx| v.set_left_tab(tab, cx)).ok();
            });
        let left_body = match self.left_tab {
            WsLeftTab::Message => self.render_composer(cx),
            WsLeftTab::Headers => self.render_headers(cx),
            WsLeftTab::Auth => self.render_auth(cx),
        };

        div()
            .id("ws-view")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(self.render_info(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("ws-left")
                            .flex()
                            .flex_col()
                            .w(relative(0.42))
                            .min_w(px(280.))
                            .border_r_1()
                            .border_color(p.border)
                            .child(div().flex_none().px(px(12.)).pt(px(8.)).child(tabs))
                            .child(left_body)
                            .test_support(),
                    )
                    .child(self.render_log(cx)),
            )
            .test_support()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn hex_parsing_and_formatting() {
        assert_eq!(parse_hex("de ad 0xBE,ef").unwrap(), vec![0xde, 0xad, 0xbe, 0xef]);
        assert!(parse_hex("abc").is_err());
        assert!(parse_hex("zz").is_err());
        let m = WsMessage {
            dir: WsDir::In,
            kind: WsKind::Text,
            data: br#"{"a":[1]}"#.to_vec(),
            at: UNIX_EPOCH + Duration::from_millis(3_723_004),
        };
        assert_eq!(clock(m.at), "01:02:03.004");
        assert!(details_text(&m).contains("\n  \"a\""));
        assert_eq!(preview(&m), r#"{"a":[1]}"#);
    }
}
