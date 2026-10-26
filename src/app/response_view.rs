//! Response pane (PLAN.md, `app::response_view`): status / time / size / schema
//! header, `Body | Headers | Timeline` sub-tabs, Pretty / Raw / Preview body modes,
//! the schema-mismatch panel, and the empty / loading / error placeholders.
//!
//! Everything expensive (pretty-printing, mismatch annotations, HTML-to-text, image
//! decoding setup) happens once in [`ResponseView::set_state`]; the resulting texts are
//! handed to the read-only body editors once, on the next render (setting an editor's
//! text needs the `Window`), and the editors lay out visible lines only.
//!
//! All text is selectable and copyable: the body (Pretty / Raw / Preview) is shown in
//! read-only code editors (selection, ⌘C, ⌘A, ⌘F search); headers, timeline, tests,
//! console, mismatches and the error description are [`SelectableText`] runs taking
//! part in the window text selection (this view mounts the [`TextSelectionLayer`] and
//! handles ⌘C for it).
//!
//! Element ids (all observable via `.test_support()`):
//! - header: `"response-header"`, `"response-status"`, `"response-time"`, `"response-size"`,
//!   `"response-schema"`, `"response-bookmark"`, `"response-error-pill"` (failed state)
//! - sub-tabs: `"tab-body"`, `"tab-headers"`, `"tab-timeline"`, `"tab-tests"`, `"tab-console"`
//!   (tab labels are exposed as the accessibility label); in the failed state with a
//!   script report: `"tab-error"`, `"tab-tests"`, `"tab-console"`
//! - tests: `"tests-list"`, `"tests-summary"`, `("test-row", i)`, `"tests-empty"`
//! - console: `"console-list"`, `("console-error", i)`, `("console-line", i)`, `"console-empty"`
//! - `"script-error-pill"` (header, post-response script failed; opens Console)
//! - body: `"body-mode"` (Segmented; children `0usize` Pretty, `1usize` Raw, `2usize` Preview),
//!   `"content-type"`, `"large-body"`, `"response-pretty"`, `"response-raw"`,
//!   `"response-preview"` (+ `"preview-image"` / `"preview-unavailable"`); each body view
//!   wraps a read-only editor ([`ResponseView::pretty_editor`], [`ResponseView::raw_editor`],
//!   [`ResponseView::preview_editor`])
//! - mismatches: `"mismatches"`, `("mismatch", i)`, `("mismatch-line", i)`
//! - headers: `"headers"`, `("header", i)`
//! - timeline: `"timeline"`, `"timeline-connection"`, `"timeline-total"`, `("timing", i)`
//!   (0 DNS, 1 Connect, 2 TLS, 3 TTFB, 4 Download, 6 "Прочее" = unaccounted rest),
//!   bars `("timing-bar", i)`; with redirects one step per hop `("hop", i)` (the final
//!   response is `("hop", hops.len())`), hop mini-bars `("hop-bar", i * 10 + phase)`;
//!   header pill `"response-redirects"` ("↪ N") opens Timeline
//! - states: `"response-empty"`, `"response-progress"`, `"response-loading"`,
//!   `"response-elapsed"`, `"cancel"`, `"response-error"` (buttons are its `"primary"` /
//!   `"secondary"` named children)

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::request_editor::{FieldHover, OffsetHoverProvider, field_hover_overlay};
use crate::core::schema_hover::{BodySide, FieldInfo, SchemaLink};
use crate::core::types::{
    LogLevel, Mismatch, ResponseData, ScriptError, ScriptReport, ScriptStage, SendError, Timings,
};
use crate::theme::{ActivePalette, MONO, Palette};
use crate::ui::{
    Button, EmptyState, ErrorState, IconButton, IconName, LoadingState, MethodBadge, Pill,
    ProgressBar, Segmented, Tooltip, icon,
};
use gpui_kit::base::{SelectableText, TextSelection, TextSelectionLayer};
use gpui_kit::component::highlighter::{Diagnostic, DiagnosticSeverity};
use gpui_kit::component::input::{
    Copy as CopySelection, Editor, EditorState, Position, RangeDecoration,
    RangeDecorationCollection, RangeDecorationStyle, RopeExt as _,
};

/// Bodies larger than this are shown Raw only (no pretty-printing / annotations).
pub const LARGE_BODY_BYTES: usize = 2 * 1024 * 1024;
const TICK: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, Default, PartialEq)]
pub enum ResponseState {
    #[default]
    Empty,
    Loading {
        host: String,
    },
    Done {
        response: ResponseData,
        /// e.g. `responses.201 → $ref Order`; `None` when the request isn't linked to a spec.
        schema_label: Option<String>,
        mismatches: Vec<Mismatch>,
    },
    Failed(SendError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResponseViewEvent {
    Retry,
    /// "Сохранить как пример" in the response header.
    SaveExample,
    Cancel,
    UpdateSchema,
    GoToSchema,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResponseTab {
    #[default]
    Body,
    Headers,
    Timeline,
    /// `pm.test` results of the last send's scripts.
    Tests,
    /// `console.*` output and script errors.
    Console,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BodyMode {
    #[default]
    Pretty,
    Raw,
    Preview,
    /// Native WebKit rendering with CSS and JavaScript.
    Browser,
}

/// Pretty-printed JSON exactly as the Pretty view shows it (2-space indent, key order
/// kept). `Mismatch::line` indices refer to lines of this string.
pub fn pretty_json(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    serde_json::to_string_pretty(&value).ok()
}

pub fn format_size(bytes: usize) -> String {
    const KB: f64 = 1024.;
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{:.1} MB", b / KB / KB)
    }
}

pub fn format_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms} ms")
    } else {
        format!("{:.1} s", d.as_secs_f64())
    }
}

fn plural_mismatches(n: usize) -> &'static str {
    match (n % 10, n % 100) {
        (_, 11..=14) => "расхождений",
        (1, _) => "расхождение",
        (2..=4, _) => "расхождения",
        _ => "расхождений",
    }
}

fn stage_title(stage: ScriptStage) -> &'static str {
    match stage {
        ScriptStage::PreRequest => "Pre-request",
        ScriptStage::PostResponse => "Post-response",
    }
}

fn script_error_text(stage: ScriptStage, err: &ScriptError) -> String {
    match err.line {
        Some(l) => format!(
            "Ошибка в {} скрипте, строка {l}: {}",
            stage_title(stage),
            err.message
        ),
        None => format!("Ошибка в {} скрипте: {}", stage_title(stage), err.message),
    }
}

/// One entry of the sub-tab strip.
struct TabItem {
    tab: ResponseTab,
    id: &'static str,
    label: String,
    /// Text color override (Tests pass/fail, dim when empty).
    color: Option<Hsla>,
    /// Small dot after the label (Console with a script error).
    dot: Option<Hsla>,
}

impl TabItem {
    fn plain(tab: ResponseTab, id: &'static str, label: String) -> Self {
        Self {
            tab,
            id,
            label,
            color: None,
            dot: None,
        }
    }
}

fn status_color(status: u16, p: &Palette) -> Hsla {
    match status {
        200..=299 => p.online,
        300..=399 => p.accent,
        400..=499 => p.warn,
        500..=599 => p.danger,
        _ => p.text_dim,
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

fn plural_steps(n: usize) -> &'static str {
    match (n % 10, n % 100) {
        (_, 11..=14) => "шагов",
        (1, _) => "шаг",
        (2..=4, _) => "шага",
        _ => "шагов",
    }
}

/// DNS + Connect + TLS + TTFB + Download (one hop's own phases).
fn phase_sum(t: &Timings) -> Duration {
    t.dns + t.connect + t.tls + t.ttfb + t.download
}

/// (label, duration, color) of one hop's phases, in order.
fn phases(t: &Timings, p: &Palette) -> [(&'static str, Duration, Hsla); 5] {
    [
        ("DNS", t.dns, p.teal),
        ("Connect", t.connect, p.purple),
        ("TLS", t.tls, p.amber),
        ("TTFB", t.ttfb, p.accent),
        ("Download", t.download, p.online),
    ]
}

/// Detailed waterfall rows `("timing", 0..4)` plus `("timing", 6)` "Прочее" for
/// the part of `total` the phases don't cover (≥ 1 ms), so the bars add up.
fn phase_rows(t: &Timings, total: Duration, p: &Palette) -> Vec<AnyElement> {
    let mut rows: Vec<(usize, &'static str, Duration, Hsla)> = phases(t, p)
        .into_iter()
        .enumerate()
        .map(|(i, (l, d, c))| (i, l, d, c))
        .collect();
    let rest = total.saturating_sub(phase_sum(t));
    if rest >= Duration::from_millis(1) {
        rows.push((6, "Прочее", rest, p.text_dim));
    }
    let total_s = total.as_secs_f32().max(f32::EPSILON);
    let mut offset = Duration::ZERO;
    rows.into_iter()
        .map(|(i, label, d, color)| {
            let left = offset.as_secs_f32() / total_s;
            let width = d.as_secs_f32() / total_s;
            offset += d;
            div()
                .id(("timing", i))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(12.))
                .h(px(24.))
                .child(
                    div()
                        .w(px(72.))
                        .flex_none()
                        .text_color(p.text_dim)
                        .child(sel("label", label)),
                )
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .h(px(8.))
                        .rounded(px(4.))
                        .bg(p.surface_hi)
                        .child(
                            div()
                                .id(("timing-bar", i))
                                .absolute()
                                .top_0()
                                .h_full()
                                .left(relative(left.clamp(0., 1.)))
                                .w(relative(width.clamp(0., 1.)))
                                .min_w(px(2.))
                                .rounded(px(4.))
                                .bg(color)
                                .test_support(),
                        ),
                )
                .child(
                    div()
                        .w(px(64.))
                        .flex_none()
                        .text_right()
                        .font_family(MONO)
                        .text_color(p.text)
                        .child(sel("duration", format_duration(d))),
                )
                .test_support()
                .into_any_element()
        })
        .collect()
}

/// One step of the redirect path: number column, then the caller's cells.
fn step_row(id: impl Into<ElementId>, n: usize, p: &Palette) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .min_w_0()
        .h(px(24.))
        .child(
            div()
                .w(px(18.))
                .flex_none()
                .font_family(MONO)
                .text_color(p.text_dim)
                .child(n.to_string()),
        )
}

fn url_cell(id: &'static str, url: &str, p: &Palette) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .text_ellipsis()
        .whitespace_nowrap()
        .font_family(MONO)
        .text_size(px(11.5))
        .text_color(p.text)
        .child(sel(id, url.to_string()))
}

fn duration_cell(d: Duration, p: &Palette) -> Div {
    div()
        .w(px(64.))
        .flex_none()
        .text_right()
        .font_family(MONO)
        .text_color(p.text)
        .child(sel("duration", format_duration(d)))
}

/// Compact single-track waterfall of one redirect hop; segments are
/// `("hop-bar", i * 10 + phase)` with phase 0 DNS … 4 Download.
fn mini_waterfall(i: usize, t: &Timings, total: Duration, p: &Palette) -> Div {
    let total_s = total.as_secs_f32().max(f32::EPSILON);
    let mut offset = Duration::ZERO;
    let segments = phases(t, p)
        .into_iter()
        .enumerate()
        .map(|(phase, (_, d, color))| {
            let left = offset.as_secs_f32() / total_s;
            let width = d.as_secs_f32() / total_s;
            offset += d;
            div()
                .id(("hop-bar", i * 10 + phase))
                .absolute()
                .top_0()
                .h_full()
                .left(relative(left.clamp(0., 1.)))
                .w(relative(width.clamp(0., 1.)))
                .when(!d.is_zero(), |s| s.min_w(px(2.)))
                .bg(color)
                .test_support()
        })
        .collect::<Vec<_>>();
    div()
        .flex()
        .flex_none()
        .pl(px(26.))
        .pr(px(72.))
        .child(
            div()
                .relative()
                .flex_1()
                .h(px(6.))
                .rounded(px(3.))
                .overflow_hidden()
                .bg(p.surface_hi)
                .children(segments),
        )
}

/// Very small HTML → text: drops tags, `<script>`/`<style>` contents, decodes the
/// common entities and collapses blank lines.
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let lower = html.to_ascii_lowercase();
    let mut i = 0;
    let bytes = html.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let rest = &lower[i..];
            for skip in ["script", "style"] {
                if rest.starts_with(&format!("<{skip}")) {
                    let close = format!("</{skip}");
                    i = rest.find(&close).map_or(bytes.len(), |e| i + e);
                    break;
                }
            }
            if i >= bytes.len() {
                break;
            }
            let tag_end = lower[i..].find('>').map_or(bytes.len(), |e| i + e + 1);
            let tag = &lower[i..tag_end];
            if [
                "<br", "<p", "</p", "<div", "</div", "<li", "<h", "</h", "<tr",
            ]
            .iter()
            .any(|t| tag.starts_with(t))
            {
                out.push('\n');
            }
            i = tag_end;
        } else {
            let next = html[i..].find('<').map_or(bytes.len(), |e| i + e);
            out.push_str(&html[i..next]);
            i = next;
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    let mut lines = Vec::new();
    for line in decoded.lines().map(str::trim) {
        if line.is_empty() && lines.last().is_none_or(|l: &&str| l.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    lines.join("\n").trim_end().to_string()
}

fn image_format(content_type: &str) -> Option<ImageFormat> {
    Some(match content_type {
        "image/png" => ImageFormat::Png,
        "image/jpeg" | "image/jpg" => ImageFormat::Jpeg,
        "image/webp" => ImageFormat::Webp,
        "image/gif" => ImageFormat::Gif,
        "image/svg+xml" => ImageFormat::Svg,
        "image/bmp" => ImageFormat::Bmp,
        "image/tiff" => ImageFormat::Tiff,
        "image/x-icon" | "image/vnd.microsoft.icon" => ImageFormat::Ico,
        _ => return None,
    })
}

enum Preview {
    /// Shown in the read-only preview editor.
    Text,
    Image(Arc<Image>),
    Unavailable,
}

/// Texts for the body editors, computed in `set_state` and applied on the next
/// render (an editor's `set_value` needs the `Window`).
struct PendingBody {
    pretty: String,
    raw: String,
    raw_language: &'static str,
    preview: String,
}

/// Everything derived from a finished response, computed once in `set_state`.
struct Prepared {
    content_type: SharedString,
    large: bool,
    /// Pretty-printed JSON is available (and shown by the pretty editor).
    pretty: bool,
    preview: Preview,
    /// 0-based pretty lines with a schema mismatch → "ожидается …" note.
    annotations: Vec<(usize, String)>,
    headers: Vec<(SharedString, SharedString)>,
    html: bool,
    /// Body as text, for the Browser mode document.
    text: String,
}

impl Prepared {
    fn new(response: &ResponseData, mismatches: &[Mismatch]) -> (Self, PendingBody) {
        let large = response.body.len() > LARGE_BODY_BYTES;
        let mime = response
            .content_type()
            .map(|c| {
                c.split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase()
            })
            .unwrap_or_default();
        let pretty_text = if large {
            None
        } else {
            pretty_json(&response.body)
        };
        let json = pretty_text.is_some() || mime.contains("json");
        let content_type: SharedString = if !mime.is_empty() {
            mime.clone().into()
        } else if json {
            "application/json".into()
        } else {
            "—".into()
        };
        let raw_string = response.body_text();
        let mut preview_text = String::new();
        let preview = if let Some(format) = image_format(&mime) {
            Preview::Image(Arc::new(Image::from_bytes(format, response.body.clone())))
        } else if mime.contains("html") {
            preview_text = html_to_text(&raw_string);
            Preview::Text
        } else if mime.starts_with("text/") || (mime.is_empty() && !json && !raw_string.is_empty())
        {
            preview_text = raw_string.clone();
            Preview::Text
        } else {
            Preview::Unavailable
        };
        let annotations = if pretty_text.is_some() {
            mismatches
                .iter()
                .filter_map(|m| {
                    let line = m.line?;
                    Some((line, format!("ожидается {}", truncate_chars(&m.expected, 60))))
                })
                .collect()
        } else {
            Vec::new()
        };
        let text = match &pretty_text {
            Some(pretty) => pretty.clone(),
            None => raw_string.clone(),
        };
        let pending = PendingBody {
            pretty: pretty_text.clone().unwrap_or_default(),
            raw_language: if json && !large { "json" } else { "text" },
            raw: raw_string,
            preview: preview_text,
        };
        let prepared = Self {
            content_type,
            large,
            pretty: pretty_text.is_some(),
            html: mime.contains("html"),
            text,
            preview,
            annotations,
            headers: response
                .headers
                .iter()
                .map(|(k, v)| (SharedString::from(k.clone()), SharedString::from(v.clone())))
                .collect(),
        };
        (prepared, pending)
    }
}

/// Selectable plain text; `id` must be unique under its nearest identified parent.
fn sel(id: impl Into<ElementId>, text: impl Into<SharedString>) -> SelectableText {
    SelectableText::new(id, text)
}

/// Byte range of 0-based `line` in `text` (without its line break).
fn line_byte_range(text: &gpui_kit::component::input::Rope, line: usize) -> Option<std::ops::Range<usize>> {
    if line >= text.lines_len() {
        return None;
    }
    let start = text.line_start_offset(line);
    let end = text.line_end_offset(line);
    Some(start..end)
}

/// Marker global: the ⌘C binding for the window text selection is registered.
struct CopyBindingInstalled;
impl Global for CopyBindingInstalled {}

pub struct ResponseView {
    focus: FocusHandle,
    state: ResponseState,
    prepared: Option<Prepared>,
    tab: ResponseTab,
    mode: BodyMode,
    highlight: Option<usize>,
    /// Read-only body editors: pretty JSON, raw body, preview text.
    pretty_editor: Entity<EditorState>,
    raw_editor: Entity<EditorState>,
    preview_editor: Entity<EditorState>,
    /// Texts waiting to be put into the editors on the next render.
    pending_body: Option<PendingBody>,
    /// Line to scroll to once the target editor has been laid out.
    pending_reveal: Option<usize>,
    /// Warn-tinted fills of the mismatch lines / the revealed line (pretty editor),
    /// and the theme mode they were painted for.
    mismatch_fill: Option<RangeDecorationCollection>,
    highlight_fill: Option<RangeDecorationCollection>,
    decor_mode: Option<crate::theme::Mode>,
    copied: Option<usize>,
    /// Host of the last `Loading` state (for error titles that carry no host).
    last_host: String,
    loading_since: Option<Instant>,
    /// Time from `Loading` to `Failed`, shown in the error pill.
    failed_after: Option<Duration>,
    _ticker: Option<Task<()>>,
    browser: super::browser::BrowserPane,
    /// URL the response came from; `<base>` for relative CSS/JS in Browser mode.
    base_url: Option<String>,
    /// Something GPUI draws (palette, popover, other rail) covers this pane, so
    /// the native webview must stay hidden.
    browser_suppressed: bool,
    /// Scripts of the last send (Tests / Console); independent of `state`.
    script_report: Option<ScriptReport>,
    /// Spec operation of the shown request (field popovers in Pretty).
    schema_link: Option<SchemaLink>,
    /// Schema popover of the Pretty field under the pointer.
    field_hover: Option<FieldHover>,
}

impl EventEmitter<ResponseViewEvent> for ResponseView {}

impl Focusable for ResponseView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ResponseView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        if !cx.has_global::<CopyBindingInstalled>() {
            cx.set_global(CopyBindingInstalled);
            cx.bind_keys([KeyBinding::new(
                "secondary-c",
                CopySelection,
                Some("ResponseView"),
            )]);
        }
        let pretty_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("json")
                .line_number(true)
        });
        let owner = cx.weak_entity();
        pretty_editor.update(cx, |s, _| {
            s.lsp_mut().hover_provider = Some(OffsetHoverProvider::provider(move |text, offset, cx| {
                owner
                    .update(cx, |this, cx| this.hover_pretty_text(text, offset, cx))
                    .ok();
            }));
        });
        let raw_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("json")
                .line_number(true)
                .soft_wrap(true)
        });
        let preview_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .line_number(false)
                .soft_wrap(true)
        });
        Self {
            pretty_editor,
            raw_editor,
            preview_editor,
            pending_body: None,
            pending_reveal: None,
            mismatch_fill: None,
            highlight_fill: None,
            decor_mode: None,
            focus: cx.focus_handle(),
            browser: Default::default(),
            base_url: None,
            browser_suppressed: false,
            script_report: None,
            schema_link: None,
            field_hover: None,
            state: ResponseState::Empty,
            prepared: None,
            tab: ResponseTab::Body,
            mode: BodyMode::Pretty,
            highlight: None,
            copied: None,
            last_host: String::new(),
            loading_since: None,
            failed_after: None,
            _ticker: None,
        }
    }

    pub fn state(&self) -> &ResponseState {
        &self.state
    }

    pub fn tab(&self) -> ResponseTab {
        self.tab
    }

    pub fn body_mode(&self) -> BodyMode {
        self.mode
    }

    /// Line highlighted from the mismatch panel.
    pub fn highlighted_line(&self) -> Option<usize> {
        self.highlight
    }

    /// `false` for non-JSON and for bodies over [`LARGE_BODY_BYTES`].
    pub fn pretty_available(&self) -> bool {
        self.prepared.as_ref().is_some_and(|p| p.pretty)
    }

    /// Read-only editor of the Pretty body view.
    pub fn pretty_editor(&self) -> &Entity<EditorState> {
        &self.pretty_editor
    }

    /// Read-only editor of the Raw body view (also Pretty for non-JSON bodies).
    pub fn raw_editor(&self) -> &Entity<EditorState> {
        &self.raw_editor
    }

    /// Read-only editor of the Preview body view (HTML as text, text bodies).
    pub fn preview_editor(&self) -> &Entity<EditorState> {
        &self.preview_editor
    }

    pub fn is_large(&self) -> bool {
        self.prepared.as_ref().is_some_and(|p| p.large)
    }

    pub fn set_state(&mut self, state: ResponseState, cx: &mut Context<Self>) {
        self.field_hover = None;
        self.highlight = None;
        self.copied = None;
        self.prepared = None;
        self._ticker = None;
        match &state {
            ResponseState::Loading { host } => {
                self.last_host = host.clone();
                self.loading_since = Some(Instant::now());
                self.failed_after = None;
                self._ticker = Some(cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor().timer(TICK).await;
                        let alive = this.update(cx, |_, cx| cx.notify()).is_ok();
                        if !alive {
                            break;
                        }
                    }
                }));
            }
            ResponseState::Done {
                response,
                mismatches,
                ..
            } => {
                let (prepared, pending) = Prepared::new(response, mismatches);
                self.pending_body = Some(pending);
                self.pending_reveal = None;
                self.mode = if prepared.large {
                    BodyMode::Raw
                } else if self.mode == BodyMode::Raw && prepared.pretty {
                    // Keep the user's Raw choice across sends; otherwise start Pretty.
                    BodyMode::Raw
                } else if matches!(self.mode, BodyMode::Preview | BodyMode::Browser) {
                    self.mode
                } else {
                    BodyMode::Pretty
                };
                self.prepared = Some(prepared);
                self.refresh_browser_document(cx);
                self.loading_since = None;
            }
            ResponseState::Failed(err) => {
                self.failed_after = self.loading_since.take().map(|t| t.elapsed());
                if let SendError::Network { host, .. } = err
                    && !host.is_empty()
                {
                    self.last_host = host.clone();
                }
            }
            ResponseState::Empty => {
                self.loading_since = None;
            }
        }
        self.state = state;
        cx.notify();
    }

    /// Spec operation of the request whose response is shown (`None`: not linked).
    pub fn set_schema_link(&mut self, link: Option<SchemaLink>, cx: &mut Context<Self>) {
        if self.schema_link != link {
            self.schema_link = link;
            self.field_hover = None;
            cx.notify();
        }
    }

    /// Schema info shown for the Pretty field under the pointer.
    pub fn field_hover(&self) -> Option<&FieldInfo> {
        self.field_hover.as_ref().map(|h| &h.info)
    }

    /// Shows the field popover for byte `offset` of the Pretty text, as a pointer
    /// resting there would (tests; the editor's hover provider ends up here too).
    pub fn hover_pretty_offset(&mut self, offset: usize, cx: &mut Context<Self>) {
        let text = self.pretty_editor.read(cx).text().to_string();
        self.hover_pretty_text(&text, offset, cx);
    }

    fn hover_pretty_text(&mut self, text: &str, offset: usize, cx: &mut Context<Self>) {
        let hover = match (&self.state, &self.schema_link) {
            (ResponseState::Done { response, mismatches, .. }, Some(link))
                if self.pretty_available() =>
            {
                FieldHover::at(link, BodySide::Response(response.status), text, offset, mismatches)
            }
            _ => None,
        };
        if self.field_hover != hover {
            self.field_hover = hover;
            cx.notify();
        }
    }

    fn clear_field_hover(&mut self, cx: &mut Context<Self>) {
        if self.field_hover.take().is_some() {
            cx.notify();
        }
    }

    /// Script results shown in the Tests / Console tabs. Kept across `set_state`.
    pub fn set_script_report(&mut self, report: Option<ScriptReport>, cx: &mut Context<Self>) {
        self.script_report = report;
        cx.notify();
    }

    pub fn script_report(&self) -> Option<&ScriptReport> {
        self.script_report.as_ref()
    }

    /// Base URL for relative links in Browser mode (the resolved request URL).
    pub fn set_base_url(&mut self, url: Option<String>, cx: &mut Context<Self>) {
        if self.base_url != url {
            self.base_url = url;
            self.refresh_browser_document(cx);
            cx.notify();
        }
    }

    /// Hide the native webview while GPUI content covers this pane.
    pub fn set_browser_suppressed(&mut self, suppressed: bool, cx: &mut Context<Self>) {
        if suppressed {
            self.browser.hide();
        }
        if self.browser_suppressed != suppressed {
            self.browser_suppressed = suppressed;
            cx.notify();
        }
    }

    /// Give the keyboard back to GPUI if the page has it (see `BrowserPane`).
    pub fn release_browser_keyboard(&self) {
        self.browser.release_keyboard();
    }

    pub fn browser_keyboard_releases(&self) -> usize {
        self.browser.keyboard_releases()
    }

    pub fn browser_visible(&self) -> bool {
        self.browser.is_visible()
    }

    fn refresh_browser_document(&self, cx: &App) {
        if let Some(prep) = &self.prepared {
            let dark = cx.palette().mode == crate::theme::Mode::Dark;
            self.browser.set_document(super::browser::browser_document(
                &prep.text,
                prep.html,
                self.base_url.as_deref(),
                dark,
            ));
        }
    }

    pub fn set_tab(&mut self, tab: ResponseTab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    pub fn set_body_mode(&mut self, mode: BodyMode, cx: &mut Context<Self>) {
        if mode == BodyMode::Pretty && self.prepared.as_ref().is_some_and(|p| p.large) {
            return;
        }
        self.mode = mode;
        cx.notify();
    }

    /// Switches to Body → Pretty, scrolls `line` (0-based) into view and highlights it.
    pub fn reveal_line(&mut self, line: usize, cx: &mut Context<Self>) {
        self.tab = ResponseTab::Body;
        if self.pretty_available() {
            self.mode = BodyMode::Pretty;
        } else if matches!(self.mode, BodyMode::Preview | BodyMode::Browser) {
            self.mode = BodyMode::Raw;
        }
        self.highlight = Some(line);
        self.pending_reveal = Some(line);
        self.paint_decorations(cx);
        cx.notify();
    }

    /// The editor `reveal_line` scrolls: Pretty when available, else Raw.
    fn reveal_target(&self) -> &Entity<EditorState> {
        if self.pretty_available() {
            &self.pretty_editor
        } else {
            &self.raw_editor
        }
    }

    /// Puts the texts prepared by `set_state` into the editors.
    fn apply_pending_body(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(body) = self.pending_body.take() else {
            return;
        };
        self.pretty_editor
            .update(cx, |s, cx| s.set_value(body.pretty, window, cx));
        self.raw_editor.update(cx, |s, cx| {
            if s.language_name().as_ref() != body.raw_language {
                s.set_highlighter(body.raw_language, cx);
            }
            s.set_value(body.raw, window, cx);
        });
        self.preview_editor
            .update(cx, |s, cx| s.set_value(body.preview, window, cx));
        // Mismatch notes as warning diagnostics (wavy underline + hover message).
        let notes = self
            .prepared
            .as_ref()
            .map(|p| p.annotations.clone())
            .unwrap_or_default();
        self.pretty_editor.update(cx, |s, cx| {
            let ranges: Vec<_> = notes
                .iter()
                .filter_map(|(line, note)| {
                    let range = line_byte_range(s.text(), *line)?;
                    let line_text = s.text().slice(range.clone()).to_string();
                    let indent = line_text.len() - line_text.trim_start().len();
                    let chars = line_text.encode_utf16().count() as u32;
                    Some((*line as u32, indent as u32, chars, note.clone()))
                })
                .collect();
            if let Some(diagnostics) = s.diagnostics_mut() {
                for (line, start, end, note) in ranges {
                    diagnostics.push(
                        Diagnostic::new(Position::new(line, start)..Position::new(line, end), note)
                            .with_severity(DiagnosticSeverity::Warning),
                    );
                }
            }
            cx.notify();
        });
        self.decor_mode = None;
        self.paint_decorations(cx);
    }

    /// (Re)paints the mismatch line fills and the revealed-line fill of the pretty editor.
    fn paint_decorations(&mut self, cx: &mut Context<Self>) {
        let p = *cx.palette();
        let lines: Vec<usize> = self
            .prepared
            .as_ref()
            .map(|prep| prep.annotations.iter().map(|(l, _)| *l).collect())
            .unwrap_or_default();
        let highlight = self.highlight.filter(|_| self.pretty_available());
        let text = self.pretty_editor.read(cx).text().clone();
        let fill = |line: usize, color: Hsla| {
            line_byte_range(&text, line).map(|r| {
                RangeDecoration::new(r)
                    .with_style(RangeDecorationStyle::Fill)
                    .with_color(color)
            })
        };
        let mismatch: Vec<_> = lines
            .iter()
            .filter_map(|&l| fill(l, p.warn.opacity(0.12)))
            .collect();
        let revealed: Vec<_> = highlight
            .into_iter()
            .filter_map(|l| fill(l, p.warn.opacity(0.28)))
            .collect();
        for (slot, decorations) in [
            (&mut self.mismatch_fill, mismatch),
            (&mut self.highlight_fill, revealed),
        ] {
            match slot {
                Some(collection) => collection.set(decorations, cx),
                None => {
                    *slot = Some(self.pretty_editor.update(cx, |s, cx| {
                        s.create_range_decorations_collection(decorations, cx)
                    }))
                }
            }
        }
        self.decor_mode = Some(p.mode);
    }

    /// Scrolls the pending revealed line to the middle of its editor, once that
    /// editor has a layout (it may never have been shown yet).
    fn apply_pending_reveal(&mut self, cx: &mut Context<Self>) {
        let Some(line) = self.pending_reveal else {
            return;
        };
        // The user moved away before the editor got a layout: nothing to scroll.
        if !matches!(self.state, ResponseState::Done { .. })
            || self.tab != ResponseTab::Body
            || !matches!(self.mode, BodyMode::Pretty | BodyMode::Raw)
        {
            self.pending_reveal = None;
            return;
        }
        let editor = self.reveal_target().clone();
        // Pretty never soft-wraps, so buffer line == display row and it can be centred;
        // the (wrapping) raw editor just scrolls the caret into view.
        let center = self.pretty_available();
        let done = editor.update(cx, |s, cx| {
            let Some(line_height) = s.line_height() else {
                return false;
            };
            let Some(range) = line_byte_range(s.text(), line) else {
                return true;
            };
            s.set_selected_range(range.start..range.start, cx);
            if !center {
                return true;
            }
            let viewport = s.input_bounds().size.height;
            let top = (line_height * line as f32 - (viewport - line_height) / 2.).max(px(0.));
            s.set_scroll_offset(point(px(0.), -top), cx);
            true
        });
        if done {
            self.pending_reveal = None;
        } else {
            // Not laid out yet: try again after the next frame.
            cx.notify();
        }
    }

    fn emitter(
        &self,
        event: ResponseViewEvent,
        cx: &mut Context<Self>,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        let this = cx.entity().downgrade();
        move |_, _, cx| {
            let ev = event.clone();
            let _ = this.update(cx, |_, cx| cx.emit(ev));
        }
    }

    /// "Отменить" button shown while loading; emits [`ResponseViewEvent::Cancel`].
    pub fn cancel_button(&self, cx: &mut Context<Self>) -> Button {
        Button::new("cancel", "Отменить")
            .icon(IconName::X)
            .kbd("Esc")
            .on_click(self.emitter(ResponseViewEvent::Cancel, cx))
    }

    // ---------------------------------------------------------------------------------

    fn header_row(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = *cx.palette();
        div()
            .id("response-header")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .h(px(40.))
            .px(px(14.))
            .border_b_1()
            .border_color(p.border)
            .text_size(px(12.))
    }

    fn render_done(
        &self,
        response: &ResponseData,
        schema_label: Option<&str>,
        mismatches: &[Mismatch],
        cx: &mut Context<Self>,
    ) -> Div {
        let p = *cx.palette();
        let Some(prep) = &self.prepared else {
            return div();
        };

        let status_label = if response.status_text.is_empty() {
            response.status.to_string()
        } else {
            format!("{} {}", response.status, response.status_text)
        };
        let schema_pill = if !mismatches.is_empty() {
            let n = mismatches.len();
            Some(Pill::new(format!("⚠ {n} {}", plural_mismatches(n))).color(p.warn))
        } else {
            schema_label.map(|l| Pill::new(format!("Схема ✓ {l}")).color(p.online))
        };

        let redirects = response
            .hops
            .len()
            .max(response.redirect_history.len().saturating_sub(1));
        let post_script_failed = self
            .script_report
            .as_ref()
            .and_then(|r| r.post.as_ref())
            .is_some_and(|o| o.error.is_some());
        let header = self
            .header_row(cx)
            .child(
                div().id("response-status").test_support().child(
                    Pill::new(status_label)
                        .mono()
                        .color(status_color(response.status, &p)),
                ),
            )
            .child(
                div()
                    .id("response-time")
                    .font_family(MONO)
                    .text_color(p.text)
                    .child(sel("text", format_duration(response.timings.total)))
                    .test_support(),
            )
            .child(
                div()
                    .id("response-size")
                    .font_family(MONO)
                    .text_color(p.text_dim)
                    .child(sel("text", format_size(response.body.len())))
                    .test_support(),
            )
            .when(redirects > 0, |d| {
                d.child(
                    div()
                        .id("response-redirects")
                        .cursor_pointer()
                        .on_click(
                            cx.listener(|this, _, _, cx| this.set_tab(ResponseTab::Timeline, cx)),
                        )
                        .child(Pill::new(format!("↪ {redirects}")))
                        .test_support(),
                )
            })
            .when_some(schema_pill, |s, pill| {
                let has_mismatch = !mismatches.is_empty();
                s.child(
                    div()
                        .id("response-schema")
                        .when(has_mismatch, |d| d.cursor_pointer())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if has_mismatch {
                                this.tab = ResponseTab::Body;
                                cx.notify();
                            }
                        }))
                        .child(pill)
                        .test_support(),
                )
            })
            .when(post_script_failed, |s| {
                s.child(
                    div()
                        .id("script-error-pill")
                        .cursor_pointer()
                        .on_click(
                            cx.listener(|this, _, _, cx| this.set_tab(ResponseTab::Console, cx)),
                        )
                        .child(Pill::new("Ошибка скрипта").color(p.warn))
                        .test_support(),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .id("response-save-example")
                    .tooltip(Tooltip::new("Сохранить как пример").part("ответ сохранится вместе с запросом").build())
                    .child(
                        IconButton::new("response-bookmark", IconName::Bookmark)
                            .on_click(self.emitter(ResponseViewEvent::SaveExample, cx)),
                    ),
            )
            .test_support();

        let tabs = self.render_tabs(prep.headers.len(), cx);

        let content = match self.tab {
            ResponseTab::Body => self
                .render_body(prep, schema_label, mismatches, cx)
                .into_any_element(),
            ResponseTab::Headers => self.render_headers(prep, response, cx).into_any_element(),
            ResponseTab::Timeline => self.render_timeline(response, cx).into_any_element(),
            ResponseTab::Tests => self.render_tests(cx),
            ResponseTab::Console => self.render_console(cx),
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(header)
            .child(tabs)
            .child(div().flex().flex_col().flex_1().min_h_0().child(content))
    }

    fn render_tabs(&self, header_count: usize, cx: &mut Context<Self>) -> Div {
        let p = *cx.palette();
        let [tests, console] = self.script_tab_items(&p);
        let items = vec![
            TabItem::plain(ResponseTab::Body, "tab-body", "Body".into()),
            TabItem::plain(
                ResponseTab::Headers,
                "tab-headers",
                format!("Headers {header_count}"),
            ),
            TabItem::plain(ResponseTab::Timeline, "tab-timeline", "Timeline".into()),
            tests,
            console,
        ];
        self.tab_strip(items, cx)
    }

    /// "Tests 3/4" (green / red, dim "Tests" when none) and "Console N" (+ red dot).
    fn script_tab_items(&self, p: &Palette) -> [TabItem; 2] {
        let report = self.script_report.as_ref();
        let (passed, total) = report.map_or((0, 0), |r| {
            r.tests()
                .fold((0, 0), |(ok, n), t| (ok + usize::from(t.passed), n + 1))
        });
        let tests = if total == 0 {
            TabItem {
                color: Some(p.text_dim.opacity(0.7)),
                ..TabItem::plain(ResponseTab::Tests, "tab-tests", "Tests".into())
            }
        } else {
            TabItem {
                color: Some(if passed == total { p.online } else { p.danger }),
                ..TabItem::plain(
                    ResponseTab::Tests,
                    "tab-tests",
                    format!("Tests {passed}/{total}"),
                )
            }
        };
        let (lines, errors) =
            report.map_or((0, 0), |r| (r.logs().count(), r.errors().count()));
        let console = TabItem {
            dot: (errors > 0).then_some(p.danger),
            ..TabItem::plain(
                ResponseTab::Console,
                "tab-console",
                format!("Console {}", lines + errors),
            )
        };
        [tests, console]
    }

    fn tab_strip(&self, items: Vec<TabItem>, cx: &mut Context<Self>) -> Div {
        let p = *cx.palette();
        div()
            .flex()
            .flex_none()
            .items_end()
            .gap(px(16.))
            .h(px(34.))
            .px(px(14.))
            .border_b_1()
            .border_color(p.border)
            .children(items.into_iter().map(|item| {
                let tab = item.tab;
                let active = self.tab == tab;
                let base = if active { p.text } else { p.text_dim };
                let color = item.color.unwrap_or(base);
                div()
                    .id(item.id)
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .h_full()
                    .text_size(px(12.))
                    .cursor_pointer()
                    .border_b_2()
                    .border_color(if active {
                        p.accent
                    } else {
                        transparent_black()
                    })
                    .text_color(color)
                    .when(!active && item.color.is_none(), |s| {
                        s.hover(|s| s.text_color(p.text))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx)))
                    .aria_label(item.label.clone())
                    .child(item.label)
                    .when_some(item.dot, |s, dot| {
                        s.child(div().size(px(6.)).rounded_full().bg(dot))
                    })
                    .test_support()
            }))
    }

    fn render_tests(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *cx.palette();
        let stages: Vec<(ScriptStage, &crate::core::types::ScriptOutcome)> = self
            .script_report
            .iter()
            .flat_map(|r| {
                r.pre
                    .iter()
                    .map(|o| (ScriptStage::PreRequest, o))
                    .chain(r.post.iter().map(|o| (ScriptStage::PostResponse, o)))
            })
            .collect();
        let total: usize = stages.iter().map(|(_, o)| o.tests.len()).sum();
        if total == 0 {
            return div()
                .size_full()
                .child(
                    EmptyState::new("tests-empty", "Нет тестов")
                        .icon(IconName::Check)
                        .hint("Добавьте pm.test(…) во вкладке Scripts запроса"),
                )
                .into_any_element();
        }
        let passed = stages
            .iter()
            .flat_map(|(_, o)| o.tests.iter())
            .filter(|t| t.passed)
            .count();
        let duration: Duration = stages.iter().map(|(_, o)| o.duration).sum();
        let summary = format!(
            "{passed} passed · {} failed · {}",
            total - passed,
            format_duration(duration)
        );
        let grouped = stages.iter().filter(|(_, o)| !o.tests.is_empty()).count() > 1;

        let mut list = div()
            .id("tests-list")
            .flex()
            .flex_col()
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .id("tests-summary")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .h(px(30.))
                    .px(px(14.))
                    .border_b_1()
                    .border_color(p.border)
                    .text_size(px(11.5))
                    .text_color(p.text_dim)
                    .aria_label(summary.clone())
                    .child(
                        div()
                            .text_color(if passed == total { p.online } else { p.danger })
                            .child(sel("text", summary)),
                    )
                    .test_support(),
            );
        let mut i = 0usize;
        for (stage, outcome) in stages.iter().filter(|(_, o)| !o.tests.is_empty()) {
            if grouped {
                list = list.child(
                    div()
                        .id(("tests-stage", *stage as usize))
                        .flex_none()
                        .px(px(14.))
                        .pt(px(10.))
                        .pb(px(4.))
                        .text_size(px(10.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.text_dim)
                        .child(sel("title", stage_title(*stage))),
                );
            }
            for t in &outcome.tests {
                let label = match (&t.error, t.passed) {
                    (_, true) => format!("✓ {}", t.name),
                    (Some(e), false) => format!("✗ {}: {e}", t.name),
                    (None, false) => format!("✗ {}", t.name),
                };
                list = list.child(
                    div()
                        .id(("test-row", i))
                        .flex()
                        .flex_none()
                        .items_start()
                        .gap(px(8.))
                        .px(px(14.))
                        .py(px(6.))
                        .border_b_1()
                        .border_color(p.border)
                        .text_size(px(12.))
                        .aria_label(label)
                        .child(div().pt(px(2.)).child(if t.passed {
                            icon(IconName::Check, 12., p.online)
                        } else {
                            icon(IconName::X, 12., p.danger)
                        }))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .flex_1()
                                .min_w_0()
                                .child(div().text_color(p.text).child(sel("name", t.name.clone())))
                                .when_some(t.error.clone().filter(|_| !t.passed), |s, e| {
                                    s.child(
                                        div()
                                            .font_family(MONO)
                                            .text_size(px(11.5))
                                            .text_color(p.danger)
                                            .child(sel("error", e)),
                                    )
                                }),
                        )
                        .test_support(),
                );
                i += 1;
            }
        }
        list.test_support().into_any_element()
    }

    fn render_console(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *cx.palette();
        let Some(report) = self
            .script_report
            .as_ref()
            .filter(|r| r.logs().next().is_some() || r.errors().next().is_some())
        else {
            return div()
                .size_full()
                .child(
                    EmptyState::new("console-empty", "Консоль пуста")
                        .hint("Здесь появится вывод console.log из скриптов"),
                )
                .into_any_element();
        };
        let errors = report.errors().enumerate().map(|(i, (stage, err))| {
            let text = script_error_text(stage, err);
            div()
                .id(("console-error", i))
                .flex()
                .flex_none()
                .items_start()
                .gap(px(8.))
                .mx(px(12.))
                .mt(px(8.))
                .px(px(10.))
                .py(px(7.))
                .rounded(px(6.))
                .border_1()
                .border_color(p.danger.opacity(0.45))
                .bg(p.danger.opacity(0.08))
                .text_size(px(12.))
                .text_color(p.danger)
                .aria_label(text.clone())
                .child(div().pt(px(1.)).child(icon(IconName::X, 12., p.danger)))
                .child(div().flex_1().min_w_0().child(sel("text", text)))
                .test_support()
        });
        let lines = report.logs().enumerate().map(|(i, (stage, line))| {
            let color = match line.level {
                LogLevel::Log => p.text,
                LogLevel::Info => p.accent,
                LogLevel::Warn => p.warn,
                LogLevel::Error => p.danger,
            };
            let tag = match stage {
                ScriptStage::PreRequest => "pre",
                ScriptStage::PostResponse => "post",
            };
            div()
                .id(("console-line", i))
                .flex()
                .flex_none()
                .items_start()
                .gap(px(8.))
                .px(px(14.))
                .py(px(3.))
                .font_family(MONO)
                .text_size(px(11.5))
                .aria_label(format!("{tag} {}", line.text))
                .child(
                    div()
                        .flex_none()
                        .w(px(34.))
                        .text_size(px(10.))
                        .text_color(p.text_dim)
                        .child(tag),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(color)
                        .child(sel("text", line.text.clone())),
                )
                .test_support()
        });
        div()
            .id("console-list")
            .flex()
            .flex_col()
            .size_full()
            .pb(px(8.))
            .overflow_y_scroll()
            .children(errors)
            .child(div().flex_none().h(px(6.)))
            .children(lines)
            .test_support()
            .into_any_element()
    }

    fn render_body(
        &self,
        prep: &Prepared,
        schema_label: Option<&str>,
        mismatches: &[Mismatch],
        cx: &mut Context<Self>,
    ) -> Div {
        let p = *cx.palette();
        let this = cx.entity().downgrade();
        let mode_ix = match self.mode {
            BodyMode::Pretty => 0,
            BodyMode::Raw => 1,
            BodyMode::Preview => 2,
            BodyMode::Browser => 3,
        };
        let toolbar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .px(px(14.))
            .py(px(8.))
            .child(
                Segmented::new("body-mode")
                    .items(["Pretty", "Raw", "Preview", "Browser"])
                    .selected(mode_ix)
                    .on_select(move |ix, _, cx| {
                        let mode = [
                            BodyMode::Pretty,
                            BodyMode::Raw,
                            BodyMode::Preview,
                            BodyMode::Browser,
                        ][ix.min(3)];
                        let _ = this.update(cx, |v, cx| v.set_body_mode(mode, cx));
                    }),
            )
            .when(prep.large, |s| {
                s.child(
                    div()
                        .id("large-body")
                        .child(Pill::new("Большой ответ · Pretty отключён").color(p.warn))
                        .test_support(),
                )
            })
            .child(div().flex_1())
            .when(self.mode == BodyMode::Browser, |s| {
                let pane = self.browser.clone();
                s.child(
                    Button::new("browser-open-external", "Открыть в браузере")
                        .ghost()
                        .icon(IconName::ExternalLink)
                        .on_click(move |_, _, cx| {
                            if let Err(e) = pane.open_external(cx) {
                                eprintln!("rivet: open in browser failed: {e}");
                            }
                        }),
                )
            })
            .child(
                div()
                    .id("content-type")
                    .font_family(MONO)
                    .text_size(px(11.))
                    .text_color(p.text_dim)
                    .child(prep.content_type.clone())
                    .test_support(),
            );

        let code: AnyElement = match self.mode {
            BodyMode::Pretty if prep.pretty => div()
                .id("response-pretty-hover")
                .size_full()
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    if !*hovered {
                        this.clear_field_hover(cx);
                    }
                }))
                .child(self.editor_view("response-pretty", &self.pretty_editor))
                .children(
                    self.field_hover
                        .as_ref()
                        .and_then(|h| field_hover_overlay(h, &self.pretty_editor, cx)),
                )
                .into_any_element(),
            BodyMode::Pretty => self.editor_view("response-pretty", &self.raw_editor),
            BodyMode::Raw => self.editor_view("response-raw", &self.raw_editor),
            BodyMode::Preview => match &prep.preview {
                Preview::Text => {
                    self.editor_view("response-preview", &self.preview_editor)
                }
                Preview::Image(image) => div()
                    .id("response-preview")
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(px(16.))
                    .child(
                        div()
                            .id("preview-image")
                            .max_w_full()
                            .max_h_full()
                            .child(
                                img(image.clone())
                                    .max_w_full()
                                    .max_h_full()
                                    .object_fit(ObjectFit::Contain),
                            )
                            .test_support(),
                    )
                    .test_support()
                    .into_any_element(),
                Preview::Unavailable => div()
                    .id("response-preview")
                    .size_full()
                    .child(
                        EmptyState::new("preview-unavailable", "Предпросмотр недоступен")
                            .icon(IconName::ImageOff),
                    )
                    .test_support()
                    .into_any_element(),
            },
            BodyMode::Browser => match self.browser.error() {
                Some(err) => div()
                    .id("response-browser")
                    .size_full()
                    .child(EmptyState::new("browser-unavailable", "Браузер недоступен").hint(err))
                    .test_support()
                    .into_any_element(),
                None => div()
                    .id("response-browser")
                    .size_full()
                    .when(!self.browser_suppressed, |d| {
                        d.child(self.browser.element())
                    })
                    .test_support()
                    .into_any_element(),
            },
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(toolbar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .border_t_1()
                    .border_color(p.border)
                    .bg(p.bg)
                    .child(code),
            )
            .when(!mismatches.is_empty(), |s| {
                s.child(self.render_mismatches(schema_label, mismatches, cx))
            })
    }

    /// A read-only body editor (selection, ⌘C, ⌘A, ⌘F) under the element `id`.
    fn editor_view(
        &self,
        id: &'static str,
        state: &Entity<EditorState>,
    ) -> AnyElement {
        div()
            .id(id)
            .size_full()
            .child(
                Editor::new(state)
                    .readonly(true)
                    .appearance(false)
                    .bordered(false)
                    .font_family(MONO)
                    .text_size(px(12.))
                    .h(relative(1.)),
            )
            .test_support()
            .into_any_element()
    }

    fn render_mismatches(
        &self,
        schema_label: Option<&str>,
        mismatches: &[Mismatch],
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let p = *cx.palette();
        let items =
            mismatches.iter().enumerate().map(|(i, m)| {
                let line = m.line;
                let selected = line.is_some() && self.highlight == line;
                div()
                    .id(("mismatch", i))
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if selected {
                        p.warn.opacity(0.6)
                    } else {
                        p.border
                    })
                    .bg(if selected {
                        p.warn.opacity(0.06)
                    } else {
                        p.surface
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .font_family(MONO)
                                    .text_size(px(11.5))
                                    .text_color(p.warn)
                                    .child(sel("path", m.path.clone())),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_color(p.text)
                                    .child(sel("title", m.title.clone())),
                            )
                            .when_some(line, |s, line| {
                                s.child(
                                    div()
                                        .id(("mismatch-line", i))
                                        .flex_none()
                                        .text_size(px(11.))
                                        .text_color(p.accent)
                                        .cursor_pointer()
                                        .hover(|s| s.text_color(p.accent_hi).underline())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.reveal_line(line, cx)
                                        }))
                                        .child(format!("стр. {}", line + 1))
                                        .test_support(),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(14.))
                            .text_size(px(11.))
                            .text_color(p.text_dim)
                            .child(
                                div().flex().gap(px(4.)).child("Ожидается").child(
                                    div()
                                        .font_family(MONO)
                                        .text_color(p.online)
                                        .child(sel("expected", truncate_chars(&m.expected, 80))),
                                ),
                            )
                            .child(
                                div().flex().gap(px(4.)).child("Получено").child(
                                    div()
                                        .font_family(MONO)
                                        .text_color(p.danger)
                                        .child(sel("actual", truncate_chars(&m.actual, 80))),
                                ),
                            ),
                    )
                    .test_support()
            });

        div()
            .id("mismatches")
            .flex()
            .flex_col()
            .flex_none()
            .max_h(px(240.))
            .gap(px(8.))
            .p(px(12.))
            .border_t_1()
            .border_color(p.border)
            .bg(p.surface)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(icon(IconName::TriangleAlert, 13., p.warn))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.text)
                            .child("Расхождения со схемой"),
                    )
                    .child(Pill::new(mismatches.len().to_string()).color(p.warn)),
            )
            .when_some(schema_label, |s, l| {
                s.child(
                    div()
                        .text_size(px(11.))
                        .text_color(p.text_dim)
                        .child(format!("Сравнение с {l}")),
                )
            })
            .child(
                div()
                    .id("mismatch-list")
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(items),
            )
            .test_support()
    }

    fn render_headers(
        &self,
        prep: &Prepared,
        response: &ResponseData,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let p = *cx.palette();
        let rows = prep.headers.iter().enumerate().map(|(i, (k, v))| {
            let copied = self.copied == Some(i);
            let value = v.clone();
            div()
                .id(("header", i))
                .group("header-row")
                .flex()
                .items_start()
                .min_h(px(28.))
                .py(px(6.))
                .px(px(14.))
                .border_b_1()
                .border_color(p.border)
                .font_family(MONO)
                .text_size(px(12.))
                .cursor_text()
                .hover(|s| s.bg(p.surface_hi))
                .on_click(cx.listener(move |this, _, window, cx| {
                    // A drag that selected text is not a click-to-copy.
                    if TextSelection::has_selection(window, cx) {
                        return;
                    }
                    cx.write_to_clipboard(ClipboardItem::new_string(value.to_string()));
                    this.copied = Some(i);
                    cx.notify();
                }))
                .child(
                    div()
                        .w(px(220.))
                        .flex_none()
                        .pr(px(10.))
                        .overflow_hidden()
                        .text_color(p.text_dim)
                        .child(sel("key", k.clone())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_color(p.text)
                        .child(sel("value", v.clone())),
                )
                .child(if copied {
                    div()
                        .flex_none()
                        .text_size(px(10.5))
                        .text_color(p.online)
                        .child("Скопировано")
                } else {
                    div()
                        .flex_none()
                        .invisible()
                        .group_hover("header-row", |s| s.visible())
                        .child(icon(IconName::Copy, 12., p.text_dim))
                })
                .test_support()
        });
        div()
            .id("headers")
            .flex()
            .flex_col()
            .size_full()
            .overflow_y_scroll()
            .when(response.redirect_history.len() > 1, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .p(px(14.))
                        .border_b_1()
                        .border_color(p.border)
                        .child(
                            div()
                                .text_size(px(11.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Цепочка редиректов"),
                        )
                        .children(
                            response
                                .redirect_history
                                .iter()
                                .enumerate()
                                .map(|(i, url)| {
                                    div()
                                        .id(("redirect-hop", i))
                                        .flex()
                                        .gap(px(8.))
                                        .min_w_0()
                                        .font_family(MONO)
                                        .text_size(px(11.))
                                        .child(
                                            div()
                                                .flex_none()
                                                .text_color(p.text_dim)
                                                .child(format!("{}.", i + 1)),
                                        )
                                        .child(
                                            div()
                                                .min_w_0()
                                                .text_color(p.text)
                                                .child(sel("url", url.clone())),
                                        )
                                        .test_support()
                                }),
                        ),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(px(26.))
                    .px(px(14.))
                    .border_b_1()
                    .border_color(p.border)
                    .text_size(px(10.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.text_dim)
                    .child(div().w(px(220.)).flex_none().child("KEY"))
                    .child(div().flex_1().child("VALUE")),
            )
            .children(rows)
            .test_support()
    }

    fn render_timeline(
        &self,
        response: &ResponseData,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let p = *cx.palette();
        let t = &response.timings;

        let mut conn = vec![response.http_version.clone()];
        if let Some(addr) = &response.remote_addr {
            conn.push(addr.clone());
        }
        if !t.tls.is_zero() {
            conn.push("TLS".into());
        }
        let conn = conn
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");

        let hops = &response.hops;
        let total = if hops.is_empty() {
            t.total.max(phase_sum(t))
        } else {
            t.total
        };
        let steps = hops.len() + 1;
        let summary = if hops.is_empty() {
            "Total".to_string()
        } else {
            format!("{steps} {} · Total", plural_steps(steps))
        };

        let mut root = div()
            .id("timeline")
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(16.))
            .size_full()
            .overflow_y_scroll()
            .text_size(px(12.))
            .child(
                div()
                    .id("timeline-connection")
                    .font_family(MONO)
                    .text_color(p.text_dim)
                    .child(sel("text", conn))
                    .test_support(),
            )
            .child(
                div()
                    .id("timeline-total")
                    .flex()
                    .gap(px(6.))
                    .mb(px(8.))
                    .aria_label(format!("{summary} {}", format_duration(total)))
                    .child(div().text_color(p.text_dim).child(sel("summary", summary)))
                    .child(
                        div()
                            .font_family(MONO)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.text)
                            .child(sel("total", format_duration(total))),
                    )
                    .test_support(),
            );

        if hops.is_empty() {
            return root.children(phase_rows(t, total, &p)).test_support();
        }

        for (i, hop) in hops.iter().enumerate() {
            let status = if hop.status_text.is_empty() {
                hop.status.to_string()
            } else {
                format!("{} {}", hop.status, hop.status_text)
            };
            let ht = &hop.timings;
            let hop_total = ht.total.max(phase_sum(ht));
            root = root.child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(px(5.))
                    .pb(px(8.))
                    .border_b_1()
                    .border_color(p.border)
                    .child(
                        step_row(("hop", i), i + 1, &p)
                            .aria_label(format!(
                                "{} {} {} → {status} → {}",
                                i + 1,
                                hop.method.label(),
                                hop.url,
                                hop.location
                            ))
                            .child(MethodBadge::new(hop.method))
                            .child(url_cell("url", &hop.url, &p))
                            .child(div().flex_none().text_color(p.text_dim).child("→"))
                            .child(
                                div().flex_none().child(
                                    Pill::new(status)
                                        .mono()
                                        .color(status_color(hop.status, &p)),
                                ),
                            )
                            .child(div().flex_none().text_color(p.text_dim).child("→"))
                            .child(url_cell("location", &hop.location, &p))
                            .child(duration_cell(hop_total, &p))
                            .test_support(),
                    )
                    .child(mini_waterfall(i, ht, hop_total, &p)),
            );
        }

        // Final response: the detailed bars of the last hop's own phases.
        let final_total = t.total.saturating_sub(t.redirect).max(phase_sum(t));
        let status = if response.status_text.is_empty() {
            response.status.to_string()
        } else {
            format!("{} {}", response.status, response.status_text)
        };
        let final_url = hops.last().map(|h| h.location.clone()).unwrap_or_default();
        root.child(
            step_row(("hop", hops.len()), steps, &p)
                .mt(px(4.))
                .aria_label(format!("{steps} Ответ {final_url} → {status}"))
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.text)
                        .child(sel("answer", "Ответ")),
                )
                .child(url_cell("url", &final_url, &p))
                .child(div().flex_none().text_color(p.text_dim).child("→"))
                .child(
                    div().flex_none().child(
                        Pill::new(status)
                            .mono()
                            .color(status_color(response.status, &p)),
                    ),
                )
                .child(div().flex_1())
                .child(duration_cell(final_total, &p))
                .test_support(),
        )
        .children(phase_rows(t, final_total, &p))
        .test_support()
    }

    fn render_loading(&self, host: &str, cx: &mut Context<Self>) -> Div {
        let p = *cx.palette();
        let elapsed = self.loading_since.map(|t| t.elapsed()).unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(ProgressBar::new("response-progress"))
            .child(
                self.header_row(cx)
                    .child(Pill::new("Отправка…").color(p.accent))
                    .child(
                        div()
                            .id("response-elapsed")
                            .font_family(MONO)
                            .text_color(p.text_dim)
                            .child(format!("{:.1} s", elapsed.as_secs_f32()))
                            .test_support(),
                    )
                    .test_support(),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .flex_none()
                            .h(px(160.))
                            .child(LoadingState::new("response-loading", host.to_string())),
                    )
                    .child(self.cancel_button(cx)),
            )
    }

    fn render_failed(&self, err: &SendError, cx: &mut Context<Self>) -> Div {
        let p = *cx.palette();
        let host = if self.last_host.is_empty() {
            "сервер".to_string()
        } else {
            self.last_host.clone()
        };
        let (code, title, description) = match err {
            SendError::Script { message, line } => (
                "SCRIPT".to_string(),
                "Pre-request скрипт завершился с ошибкой".to_string(),
                match line {
                    Some(l) => format!("Строка {l}: {message}. Запрос не отправлен."),
                    None => format!("{message}. Запрос не отправлен."),
                },
            ),
            SendError::Network {
                code,
                message,
                host: h,
            } => {
                let h = if h.is_empty() {
                    host.clone()
                } else {
                    h.clone()
                };
                let upper = code.to_ascii_uppercase();
                let (title, desc) = if upper == "ECONNREFUSED" {
                    (
                        format!("Could not connect to {h}"),
                        "Сервер не отвечает: соединение отклонено. Проверьте, что он запущен и порт указан верно.".to_string(),
                    )
                } else if upper == "ENOTFOUND" || upper.contains("DNS") {
                    (
                        format!("Could not resolve {h}"),
                        format!(
                            "DNS не нашёл адрес «{h}». Проверьте имя хоста, переменную baseUrl и подключение к сети."
                        ),
                    )
                } else if upper.contains("TLS") || upper.contains("SSL") || upper.contains("CERT") {
                    (
                        format!("TLS handshake with {h} failed"),
                        format!(
                            "Не удалось установить защищённое соединение: {message}. Проверьте сертификат или отключите проверку SSL в настройках."
                        ),
                    )
                } else {
                    (
                        format!("Could not connect to {h}"),
                        if message.is_empty() {
                            code.clone()
                        } else {
                            message.clone()
                        },
                    )
                };
                (code.clone(), title, desc)
            }
            SendError::Timeout { after_ms } => (
                "TIMEOUT".to_string(),
                format!("{host} did not respond in time"),
                format!(
                    "Сервер не ответил за {}. Увеличьте таймаут в настройках или повторите запрос.",
                    format_duration(Duration::from_millis(*after_ms))
                ),
            ),
            SendError::InvalidUrl(u) => (
                "INVALID_URL".to_string(),
                "Invalid URL".to_string(),
                format!(
                    "Не удалось разобрать адрес «{u}». Проверьте схему (http/https) и переменные окружения."
                ),
            ),
            SendError::File { path, message } => (
                "FILE".to_string(),
                "Не удалось прочитать файл".to_string(),
                format!("{path}: {message}"),
            ),
            SendError::InvalidBody(message) => (
                "INVALID_BODY".to_string(),
                "Тело запроса не собрано".to_string(),
                format!("{message}. Запрос не отправлен: исправьте тело во вкладке Body."),
            ),
            SendError::InvalidHeader { name, message } => (
                "INVALID_HEADER".to_string(),
                format!("Недопустимый заголовок «{name}»"),
                format!("{message}. Запрос не отправлен: исправьте заголовок во вкладке Headers."),
            ),
            SendError::Cancelled => (
                "CANCELLED".to_string(),
                "Запрос отменён".to_string(),
                "Отправка была прервана. Нажмите «Повторить», чтобы отправить снова.".to_string(),
            ),
        };
        let pill = match self.failed_after {
            Some(d) => format!("Error {code} · {}", format_duration(d)),
            None => format!("Error {code}"),
        };
        let has_report = self.script_report.as_ref().is_some_and(|r| !r.is_empty());
        let tabs = has_report.then(|| {
            let [tests, console] = self.script_tab_items(&p);
            let error = TabItem::plain(ResponseTab::Body, "tab-error", "Ошибка".into());
            // Body / Headers / Timeline make no sense here: they all show the error.
            self.tab_strip(vec![error, tests, console], cx)
        });
        let content = match self.tab {
            ResponseTab::Tests if has_report => self.render_tests(cx),
            ResponseTab::Console if has_report => self.render_console(cx),
            _ => ErrorState::new("response-error", title)
                .icon(IconName::Unplug)
                .description(description)
                .primary_action("Повторить", self.emitter(ResponseViewEvent::Retry, cx))
                .secondary_action("Настройки прокси", |_, _, _| {})
                .into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                self.header_row(cx)
                    .child(
                        div()
                            .id("response-error-pill")
                            .child(Pill::new(pill).mono().color(p.danger))
                            .test_support(),
                    )
                    .test_support(),
            )
            .children(tabs)
            .child(div().flex_1().min_h_0().child(content))
    }
}

impl Render for ResponseView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.apply_pending_body(window, cx);
        if self.decor_mode.is_some_and(|m| m != cx.palette().mode) {
            self.paint_decorations(cx);
        }
        self.apply_pending_reveal(cx);
        let browser_on_screen = !self.browser_suppressed
            && self.tab == ResponseTab::Body
            && self.mode == BodyMode::Browser
            && matches!(self.state, ResponseState::Done { .. });
        if !browser_on_screen {
            self.browser.hide();
        }
        let p = *cx.palette();
        let content = match &self.state {
            ResponseState::Empty => div()
                .size_full()
                .child(
                    EmptyState::new("response-empty", "Ответ появится здесь")
                        .icon(IconName::SendHorizontal)
                        .hint("Нажмите {kbd}, чтобы отправить запрос")
                        .kbd(crate::ui::shortcut_hint("⌘↵", "Ctrl+Enter")),
                )
                .into_any_element(),
            ResponseState::Loading { host } => self.render_loading(host, cx).into_any_element(),
            ResponseState::Done {
                response,
                schema_label,
                mismatches,
            } => self
                .render_done(response, schema_label.as_deref(), mismatches, cx)
                .into_any_element(),
            ResponseState::Failed(err) => self.render_failed(err, cx).into_any_element(),
        };
        div()
            .id("response-view")
            .track_focus(&self.focus)
            .key_context("ResponseView")
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape"
                    && matches!(this.state, ResponseState::Loading { .. })
                {
                    cx.stop_propagation();
                    cx.emit(ResponseViewEvent::Cancel);
                }
            }))
            .on_action(cx.listener(|_, _: &CopySelection, window, cx| {
                // ⌘C over the window text selection (the editors handle their own).
                let text = TextSelection::selected_text(window, cx);
                if text.is_empty() {
                    cx.propagate();
                } else {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if !this.focus.contains_focused(window, cx) {
                        window.focus(&this.focus, cx);
                    }
                }),
            )
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .bg(p.bg)
            .text_color(p.text)
            // Enables the window text selection for the SelectableText runs below.
            .child(TextSelectionLayer)
            .child(content)
            .test_support()
    }
}
