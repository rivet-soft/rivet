//! Request editor (PLAN.md, `app::request_editor`): Params / Headers / Auth above
//! an always-visible Body pane. Every edit is written straight into the
//! spec and announced with [`RequestEditorEvent::Changed`].
//!
//! Key-value tables are Postman style: the last row is always an empty draft;
//! typing into it turns it into a real row and appends a fresh draft.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::highlighter::{Diagnostic, DiagnosticSeverity};
use gpui_kit::component::input::{
    Editor, EditorState, HoverProvider, Input, InputEvent, InputState, RangeDecoration,
    RangeDecorationCollection, RangeDecorationStyle, Rope, RopeExt as _, TextDecoration,
    TextDecorationCollection, Textarea, TextareaState,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::core::openapi::GeneratedRequestBody;
use crate::core::bulk;
use crate::core::header_check::{self, HeaderIssue};
use crate::core::schema_hover::{self, BodySide, FieldInfo, SchemaLink};
use crate::core::vars;
use crate::core::types::{Auth, BodyKind, KeyValue, Mismatch, RequestSpec};
use crate::theme::{ActivePalette, MONO};
use crate::ui::{
    BodyModePicker, Button, FormFieldTypePicker, HorizontalPaneSplitter, IconButton, IconName,
    PANE_SPLITTER_HEIGHT, Pill, Popover, RequestRedirectSettings, Segmented, Tooltip, icon,
    tokenize_vars, var_content, var_token_element,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestEditorEvent {
    /// The spec was edited; read it with [`RequestEditor::spec`].
    Changed,
    /// "✦ Generate from schema" was clicked; answer with [`RequestEditor::set_generated_body`].
    GenerateBody,
    /// The panel's "Save" button was clicked.
    Save,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RequestEditorTab {
    #[default]
    Params,
    Headers,
    Body,
    Auth,
    Settings,
    /// Declared last so the existing `("subtab", n)` ids keep their numbers;
    /// shown right after Auth.
    Scripts,
    /// The request's description (documentation), shown after Scripts.
    Docs,
}

impl RequestEditorTab {
    pub const ALL: [RequestEditorTab; 7] = [
        Self::Params,
        Self::Headers,
        Self::Body,
        Self::Auth,
        Self::Scripts,
        Self::Docs,
        Self::Settings,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Params => "Params",
            Self::Headers => "Headers",
            Self::Body => "Body",
            Self::Auth => "Auth",
            Self::Settings => "Settings",
            Self::Scripts => "Scripts",
            Self::Docs => "Docs",
        }
    }
}

/// Which script the Scripts tab edits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScriptStage {
    #[default]
    PreRequest,
    PostResponse,
}

impl ScriptStage {
    pub const ALL: [ScriptStage; 2] = [Self::PreRequest, Self::PostResponse];

    fn label(self) -> &'static str {
        match self {
            Self::PreRequest => "Pre-request",
            Self::PostResponse => "Post-response",
        }
    }

    /// Snippets offered for this stage: `(title, code)`.
    pub fn snippets(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::PreRequest => PRE_REQUEST_SNIPPETS,
            Self::PostResponse => POST_RESPONSE_SNIPPETS,
        }
    }

    fn script(self, spec: &RequestSpec) -> &str {
        match self {
            Self::PreRequest => &spec.scripts.pre_request,
            Self::PostResponse => &spec.scripts.post_response,
        }
    }

    fn script_mut(self, spec: &mut RequestSpec) -> &mut String {
        match self {
            Self::PreRequest => &mut spec.scripts.pre_request,
            Self::PostResponse => &mut spec.scripts.post_response,
        }
    }
}

const PRE_REQUEST_SNIPPETS: &[(&str, &str)] = &[
    (
        "Задать переменную",
        "pm.environment.set(\"timestamp\", Date.now().toString());",
    ),
    (
        "Добавить заголовок",
        "pm.request.headers.upsert({ key: \"X-Request-Id\", value: pm.variables.replaceIn(\"{{$uuid}}\") });",
    ),
    ("Лог запроса", "console.log(pm.request.url.toString());"),
];

const POST_RESPONSE_SNIPPETS: &[(&str, &str)] = &[
    (
        "Сохранить поле ответа в окружение",
        "const json = pm.response.json();\npm.environment.set(\"token\", json.token);",
    ),
    (
        "Статус 200",
        "pm.test(\"Status is 200\", () => {\n  pm.response.to.have.status(200);\n});",
    ),
    (
        "Время ответа < 500 мс",
        "pm.test(\"Response time < 500ms\", () => {\n  pm.expect(pm.response.responseTime).to.be.below(500);\n});",
    ),
    (
        "Поле есть в ответе",
        "pm.test(\"Has id\", () => {\n  pm.expect(pm.response.json()).to.have.property(\"id\");\n});",
    ),
    ("Лог ответа", "console.log(pm.response.json());"),
];

/// Auth type picker entries, in menu order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthKind {
    Inherit,
    Bearer,
    Basic,
    ApiKey,
    None,
}

impl AuthKind {
    pub const ALL: [AuthKind; 5] = [
        Self::Inherit,
        Self::Bearer,
        Self::Basic,
        Self::ApiKey,
        Self::None,
    ];

    fn of(auth: &Auth) -> Self {
        match auth {
            Auth::None => Self::None,
            Auth::Inherit => Self::Inherit,
            Auth::Bearer { .. } => Self::Bearer,
            Auth::Basic { .. } => Self::Basic,
            Auth::ApiKey { .. } => Self::ApiKey,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Inherit => "Inherit from spec",
            Self::Bearer => "Bearer Token",
            Self::Basic => "Basic Auth",
            Self::ApiKey => "API Key",
            Self::None => "No auth",
        }
    }
}

const BODY_KINDS: [BodyKind; 6] = [
    BodyKind::None,
    BodyKind::Multipart,
    BodyKind::Form,
    BodyKind::Json,
    BodyKind::Text,
    BodyKind::Binary,
];
const MIN_DETAILS_HEIGHT: f32 = 140.;
const MIN_BODY_HEIGHT: f32 = 170.;

fn details_height(available: f32, ratio: f32) -> f32 {
    let usable = (available - PANE_SPLITTER_HEIGHT).max(0.);
    let details_min = MIN_DETAILS_HEIGHT.min(usable / 2.);
    let body_min = MIN_BODY_HEIGHT.min(usable / 2.);
    (usable * ratio).clamp(details_min, usable - body_min)
}

/// Which key-value list of the spec a table edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum KvTarget {
    Params,
    /// `:name` path segments of the URL; keys follow the URL, rows are never
    /// added or removed by hand.
    PathVars,
    Headers,
    Form,
}

impl KvTarget {
    fn scope(self) -> &'static str {
        match self {
            Self::Params => "req-params",
            Self::Headers => "req-headers",
            Self::Form => "req-form",
            Self::PathVars => "req-path-vars",
        }
    }

    fn list(self, spec: &RequestSpec) -> &Vec<KeyValue> {
        match self {
            Self::Params => &spec.params,
            Self::Headers => &spec.headers,
            Self::Form => &spec.body.form,
            Self::PathVars => &spec.path_vars,
        }
    }

    fn list_mut(self, spec: &mut RequestSpec) -> &mut Vec<KeyValue> {
        match self {
            Self::Params => &mut spec.params,
            Self::Headers => &mut spec.headers,
            Self::Form => &mut spec.body.form,
            Self::PathVars => &mut spec.path_vars,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Key,
    Value,
    Description,
}

#[derive(Clone, Copy)]
enum FileTarget {
    NewFormField,
    FormField(u64),
    BinaryBody,
}

/// A long value cell opened as a wrapping, multi-line editor over the rows
/// below while it has focus (like Postman); edits go to the row's value.
struct ExpandedValue {
    target: KvTarget,
    row_id: u64,
    state: Entity<TextareaState>,
    _subscription: Subscription,
}

/// Approximate width of `text` in the 12px monospace cell font.
fn mono_text_width(text: &str) -> f32 {
    text.chars().count() as f32 * 7.25
}

/// Params / Headers edited as `key:value` text (bulk edit).
struct BulkEdit {
    target: KvTarget,
    editor: Entity<EditorState>,
    /// Rows when the text was loaded: descriptions are carried over from them.
    original: Vec<KeyValue>,
    _subscription: Subscription,
}

impl KvTarget {
    fn of_tab(tab: RequestEditorTab) -> Option<Self> {
        match tab {
            RequestEditorTab::Params => Some(Self::Params),
            RequestEditorTab::Headers => Some(Self::Headers),
            RequestEditorTab::Body => Some(Self::Form),
            _ => None,
        }
    }
}

/// Editors of one table row. Dropping it drops its subscriptions.
struct RowInputs {
    id: u64,
    key: Entity<InputState>,
    value: Entity<InputState>,
    description: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

/// `eyJh••••3fQ`: enough to recognise a secret without showing it.
pub fn mask_secret(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    if chars.is_empty() {
        String::new()
    } else if chars.len() <= 8 {
        "••••••••".into()
    } else {
        let head: String = chars[..4].iter().collect();
        let tail: String = chars[chars.len() - 3..].iter().collect();
        format!("{head}••••{tail}")
    }
}

/// Pretty-prints JSON with 2-space indentation, keeping key order.
pub fn beautify_json(text: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    serde_json::to_string_pretty(&value).map_err(|e| e.to_string())
}

/// Bodies above this size are not checked live (typing stays instant).
const LIVE_CHECK_LIMIT: usize = 512 * 1024;

/// One live-validation finding in the body editor.
#[derive(Clone, Debug, PartialEq)]
pub struct BodyProblem {
    pub range: Range<usize>,
    pub severity: DiagnosticSeverity,
    pub message: String,
}

/// Schema popover state of a JSON editor: the hovered token and what the spec says.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FieldHover {
    /// Byte range of the hovered key / value (popover anchor).
    pub range: Range<usize>,
    pub info: FieldInfo,
    /// Schema mismatch at this path: (expected, actual).
    pub mismatch: Option<(String, String)>,
}

impl FieldHover {
    /// Field under `offset` of a JSON `text`, described by the linked operation.
    pub(crate) fn at(
        link: &SchemaLink,
        side: BodySide,
        text: &str,
        offset: usize,
        mismatches: &[Mismatch],
    ) -> Option<Self> {
        let (range, path) = schema_hover::json_token_at(text, offset)?;
        if path.is_empty() {
            return None;
        }
        let info = schema_hover::field_info(&link.spec, &link.spec_ref, side, &path)?;
        let at = schema_hover::path_to_string(&path);
        let mismatch = mismatches
            .iter()
            .find(|m| m.path == at)
            .map(|m| (m.expected.clone(), m.actual.clone()));
        Some(Self {
            range,
            info,
            mismatch,
        })
    }
}

/// A [`HoverProvider`] that never answers with LSP content: it only reports the
/// byte offset under the pointer (after the editor's hover delay) to its owner,
/// which renders its own styled popover ([`field_hover_card`]). The built-in hover
/// popover can only show markdown, not the chips of the design.
pub(crate) struct OffsetHoverProvider {
    on_hover: Box<OnHoverOffset>,
}

/// `(editor text, byte offset under the pointer, cx)`.
type OnHoverOffset = dyn Fn(&str, usize, &mut App);

impl OffsetHoverProvider {
    pub(crate) fn provider(f: impl Fn(&str, usize, &mut App) + 'static) -> Rc<dyn HoverProvider> {
        Rc::new(Self {
            on_hover: Box::new(f),
        })
    }
}

impl HoverProvider for OffsetHoverProvider {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Option<lsp_types::Hover>>> {
        (self.on_hover)(&text.to_string(), offset, cx);
        Task::ready(Ok(None))
    }
}

fn hover_chip(id: &'static str, text: impl Into<SharedString>) -> Stateful<Div> {
    let text = text.into();
    div()
        .id(id)
        .flex_none()
        .px(px(6.))
        .py(px(1.))
        .rounded(px(4.))
        .font_family(MONO)
        .text_size(px(10.5))
        .aria_label(text.clone())
        .child(text)
}

/// The schema popover: name + type / required / location chips + constraints,
/// description, optional mismatch warning, schema path footer.
///
/// Ids: `"field-hover"` (card, labelled with the field name), `"field-hover-type"`,
/// `"field-hover-required"`, `"field-hover-location"`, `"field-hover-constraints"`,
/// `"field-hover-description"`, `"field-hover-mismatch"`, `"field-hover-path"`.
pub(crate) fn field_hover_card(
    info: &FieldInfo,
    mismatch: Option<&(String, String)>,
    p: &crate::theme::Palette,
) -> AnyElement {
    let header = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(6.))
        .child(
            div()
                .font_family(MONO)
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(12.5))
                .text_color(p.text)
                .child(info.name.clone()),
        )
        .child(
            hover_chip("field-hover-type", info.type_label.clone())
                .bg(p.amber.opacity(0.16))
                .text_color(p.amber)
                .test_support(),
        )
        .when(info.required, |d| {
            d.child(
                hover_chip("field-hover-required", "required")
                    .border_1()
                    .border_color(p.danger.opacity(0.6))
                    .text_color(p.danger)
                    .test_support(),
            )
        })
        .when_some(info.location.clone(), |d, loc| {
            d.child(
                hover_chip("field-hover-location", loc)
                    .bg(p.teal.opacity(0.14))
                    .text_color(p.teal)
                    .test_support(),
            )
        })
        .when(!info.constraints.is_empty(), |d| {
            let label = info.constraints.join("  ");
            d.child(div().flex_1().min_w(px(8.))).child(
                div()
                    .id("field-hover-constraints")
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .font_family(MONO)
                    .text_size(px(10.5))
                    .text_color(p.text_dim)
                    .aria_label(label)
                    .children(info.constraints.iter().map(|c| div().child(c.clone())))
                    .test_support(),
            )
        });
    let description = info.description.clone().map(|text| {
        div()
            .id("field-hover-description")
            .text_size(px(12.))
            .line_height(px(17.))
            .text_color(p.text)
            .aria_label(text.clone())
            .child(text)
            .test_support()
    });
    let warn = mismatch.map(|(expected, actual)| {
        let text = format!("Ожидается {expected} · Получено {actual}");
        div()
            .id("field-hover-mismatch")
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(8.))
            .py(px(6.))
            .rounded(px(6.))
            .bg(p.warn.opacity(0.10))
            .border_1()
            .border_color(p.warn.opacity(0.35))
            .text_size(px(11.5))
            .text_color(p.warn)
            .aria_label(text.clone())
            .child(icon(IconName::TriangleAlert, 12., p.warn))
            .child(div().min_w_0().child(text))
            .test_support()
    });
    div()
        .id("field-hover")
        .flex()
        .flex_col()
        .gap(px(8.))
        .w(px(380.))
        .p(px(12.))
        .bg(p.surface)
        .border_1()
        .border_color(p.border)
        .rounded(px(10.))
        .shadow(vec![BoxShadow {
            color: black().opacity(0.28),
            offset: point(px(0.), px(8.)),
            blur_radius: px(24.),
            spread_radius: px(0.),
            inset: false,
        }])
        .aria_label(info.name.clone())
        .child(header)
        .children(description)
        .children(warn)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .pt(px(8.))
                .border_t_1()
                .border_color(p.border)
                .font_family(MONO)
                .text_size(px(10.5))
                .text_color(p.text_dim)
                .child(
                    div()
                        .id("field-hover-path")
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .aria_label(info.schema_path.clone())
                        .child(info.schema_path.clone())
                        .test_support(),
                )
                .child(div().flex_none().child("из схемы")),
        )
        .test_support()
        .into_any_element()
}

/// The card floating just under `hover.range` of `editor` (window coordinates), or
/// nothing while that range is scrolled out of view.
pub(crate) fn field_hover_overlay(
    hover: &FieldHover,
    editor: &Entity<EditorState>,
    cx: &App,
) -> Option<AnyElement> {
    let bounds = editor.read(cx).range_to_bounds(&hover.range)?;
    let p = *cx.palette();
    Some(
        deferred(
            anchored()
                .position(point(bounds.left(), bounds.bottom() + px(4.)))
                .snap_to_window_with_margin(px(8.))
                .child(field_hover_card(&hover.info, hover.mismatch.as_ref(), &p)),
        )
        .with_priority(2)
        .into_any_element(),
    )
}

fn plural_ru(n: usize, one: &'static str, few: &'static str, many: &'static str) -> &'static str {
    let (m10, m100) = (n % 10, n % 100);
    if m10 == 1 && m100 != 11 {
        one
    } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
        few
    } else {
        many
    }
}

pub struct RequestEditor {
    spec: RequestSpec,
    tab: RequestEditorTab,
    split_ratio: f32,
    split_dragging: bool,
    split_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    params: Vec<RowInputs>,
    headers: Vec<RowInputs>,
    form: Vec<RowInputs>,
    path_vars: Vec<RowInputs>,
    next_row_id: u64,
    /// Tables currently shown as bulk text instead of rows.
    bulk: Vec<BulkEdit>,
    /// The value cell currently expanded for editing, if any.
    expanded: Option<ExpandedValue>,
    /// Last painted width of each value cell: `(table, row id)` → px.
    value_widths: Rc<std::cell::RefCell<std::collections::HashMap<(KvTarget, u64), f32>>>,
    body_editor: Entity<EditorState>,
    body_error: Option<String>,
    /// Operation of a loaded spec this request is linked to (hover info, schema checks).
    schema_link: Option<SchemaLink>,
    /// Schema popover of the body field under the pointer.
    field_hover: Option<FieldHover>,
    /// Params / Headers key cell under the pointer: (table, row).
    param_hover: Option<(KvTarget, usize)>,
    /// Live JSON check of the body: syntax error / schema warnings, in text order.
    body_problems: Vec<BodyProblem>,
    /// Schema mismatches behind the warnings (for the hover warn section).
    body_mismatches: Vec<Mismatch>,
    /// The body was checked (JSON mode, not blank): the summary line is shown.
    body_checked: bool,
    /// `{{var}}` highlighting in the body editor: text colour + tinted fill,
    /// and the theme mode they were painted for.
    body_vars: Option<(
        TextDecorationCollection,
        RangeDecorationCollection,
        crate::theme::Mode,
    )>,
    auth_kind: AuthKind,
    auth_menu_open: bool,
    token_revealed: bool,
    api_in_query: bool,
    token: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    api_key: Entity<InputState>,
    api_value: Entity<InputState>,
    redirect_limit: Entity<InputState>,
    /// Timeout override in ms ("" = the app's).
    request_timeout: Entity<InputState>,
    /// App timeout (ms) and SSL check, shown next to the overrides.
    connection_defaults: (u64, bool),
    redirect_defaults: (bool, u32),
    script_stage: ScriptStage,
    /// One JS editor per stage, so each keeps its own cursor and undo history.
    pre_script: Entity<EditorState>,
    /// Docs tab: the request description.
    docs_editor: Entity<EditorState>,
    post_script: Entity<EditorState>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<RequestEditorEvent> for RequestEditor {}

impl RequestEditor {
    pub fn new(spec: RequestSpec, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let body_editor = cx.new(|cx| EditorState::new(window, cx).language("json"));
        let owner = cx.weak_entity();
        body_editor.update(cx, |s, _| {
            s.lsp_mut().hover_provider = Some(OffsetHoverProvider::provider(move |text, offset, cx| {
                owner
                    .update(cx, |this, cx| this.hover_body_text(text, offset, cx))
                    .ok();
            }));
        });
        let token = cx.new(|cx| InputState::new(window, cx).placeholder("{{token}}"));
        let username = cx.new(|cx| InputState::new(window, cx).placeholder("Username"));
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Password")
                .masked(true)
        });
        let api_key = cx.new(|cx| InputState::new(window, cx).placeholder("X-API-Key"));
        let api_value = cx.new(|cx| InputState::new(window, cx).placeholder("Value"));
        let redirect_limit = cx.new(|cx| InputState::new(window, cx).placeholder("По умолчанию"));
        let request_timeout = cx.new(|cx| InputState::new(window, cx).placeholder("По умолчанию"));
        let pre_script = cx.new(|cx| EditorState::new(window, cx).language("javascript"));
        let post_script = cx.new(|cx| EditorState::new(window, cx).language("javascript"));
        let docs_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .line_number(false)
                .soft_wrap(true)
                .placeholder("Что делает запрос, параметры, примеры, ссылки (Markdown)")
        });

        let mut subscriptions = vec![cx.subscribe_in(
            &body_editor,
            window,
            |this, state, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    this.spec.body.text = state.read(cx).value().to_string();
                    this.highlight_body_vars(cx);
                    this.body_error = None;
                    this.field_hover = None;
                    this.refresh_body_diagnostics(cx);
                    cx.emit(RequestEditorEvent::Changed);
                    cx.notify();
                }
            },
        )];
        for (state, tokens) in [
            (&token, true),
            (&username, true),
            (&password, false),
            (&api_key, false),
            (&api_value, true),
        ] {
            subscriptions.push(cx.subscribe_in(
                state,
                window,
                move |this, state, ev: &InputEvent, window, cx| {
                    if let InputEvent::Change = ev {
                        if tokens {
                            state.update(cx, |s, cx| tokenize_vars(s, window, cx));
                        }
                        this.sync_auth(cx);
                    }
                },
            ));
        }
        subscriptions.push(cx.subscribe_in(
            &request_timeout,
            window,
            |this, state, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    let value = state.read(cx).value().trim().to_string();
                    // Empty: the app's timeout; otherwise 1 ms .. 1 hour.
                    let parsed = if value.is_empty() {
                        Some(None)
                    } else {
                        value.parse::<u64>().ok().filter(|v| (1..=3_600_000).contains(v)).map(Some)
                    };
                    if let Some(timeout) = parsed {
                        if this.spec.timeout_ms != timeout {
                            this.spec.timeout_ms = timeout;
                            cx.emit(RequestEditorEvent::Changed);
                        }
                        cx.notify();
                    }
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &redirect_limit,
            window,
            |this, state, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    let value = state.read(cx).value().trim().to_string();
                    let parsed = if value.is_empty() {
                        Some(None)
                    } else {
                        value
                            .parse::<u32>()
                            .ok()
                            .filter(|v| (1..=50).contains(v))
                            .map(Some)
                    };
                    if let Some(limit) = parsed {
                        this.spec.max_redirects = limit;
                        cx.emit(RequestEditorEvent::Changed);
                        cx.notify();
                    }
                }
            },
        ));
        for (state, stage) in [
            (&pre_script, ScriptStage::PreRequest),
            (&post_script, ScriptStage::PostResponse),
        ] {
            subscriptions.push(cx.subscribe_in(
                state,
                window,
                move |this, state, ev: &InputEvent, _, cx| {
                    if let InputEvent::Change = ev {
                        let text = state.read(cx).value().to_string();
                        let slot = stage.script_mut(&mut this.spec);
                        if *slot != text {
                            *slot = text;
                            cx.emit(RequestEditorEvent::Changed);
                        }
                        cx.notify();
                    }
                },
            ));
        }

        subscriptions.push(cx.subscribe_in(
            &docs_editor,
            window,
            |this, state, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    let text = state.read(cx).value().to_string();
                    if this.spec.description != text {
                        this.spec.description = text;
                        cx.emit(RequestEditorEvent::Changed);
                    }
                    cx.notify();
                }
            },
        ));
        let mut this = Self {
            spec: spec.clone(),
            tab: RequestEditorTab::default(),
            split_ratio: 0.48,
            split_dragging: false,
            split_bounds: Rc::new(Cell::new(None)),
            params: Vec::new(),
            headers: Vec::new(),
            form: Vec::new(),
            path_vars: Vec::new(),
            next_row_id: 0,
            bulk: Vec::new(),
            expanded: None,
            value_widths: Default::default(),
            body_editor,
            body_error: None,
            schema_link: None,
            field_hover: None,
            param_hover: None,
            body_problems: Vec::new(),
            body_mismatches: Vec::new(),
            body_checked: false,
            body_vars: None,
            auth_kind: AuthKind::None,
            auth_menu_open: false,
            token_revealed: false,
            api_in_query: false,
            token,
            username,
            password,
            api_key,
            api_value,
            redirect_limit,
            redirect_defaults: (true, 10),
            connection_defaults: (30_000, true),
            request_timeout,
            script_stage: ScriptStage::default(),
            pre_script,
            post_script,
            docs_editor,
            _subscriptions: subscriptions,
        };
        this.set_spec(spec, window, cx);
        this
    }

    pub fn spec(&self) -> &RequestSpec {
        &self.spec
    }

    /// Loads another request, rebuilding every editor. Emits nothing.
    pub fn set_spec(&mut self, spec: RequestSpec, window: &mut Window, cx: &mut Context<Self>) {
        self.spec = spec;
        self.spec.path_vars = vars::sync_path_vars(&self.spec.url, &self.spec.path_vars);
        self.expanded = None;
        self.body_error = None;
        self.auth_menu_open = false;
        for target in [KvTarget::Params, KvTarget::Headers, KvTarget::Form] {
            let rows: Vec<RowInputs> = target
                .list(&self.spec)
                .clone()
                .iter()
                .map(Some)
                .chain([None])
                .map(|kv| self.make_row(target, kv, window, cx))
                .collect();
            *self.rows_mut(target) = rows;
        }
        self.rebuild_path_var_rows(window, cx);
        for ix in 0..self.bulk.len() {
            let list = self.bulk[ix].target.list(&self.spec).clone();
            let text = bulk::to_text(&list);
            self.bulk[ix].original = list;
            self.bulk[ix]
                .editor
                .clone()
                .update(cx, |s, cx| s.set_value(text, window, cx));
        }

        let text = self.spec.body.text.clone();
        self.body_editor
            .update(cx, |s, cx| s.set_value(text, window, cx));
        self.highlight_body_vars(cx);
        self.field_hover = None;
        self.param_hover = None;
        self.refresh_body_diagnostics(cx);

        let docs = self.spec.description.clone();
        self.docs_editor.update(cx, |s, cx| s.set_value(docs, window, cx));
        for stage in ScriptStage::ALL {
            let text = stage.script(&self.spec).to_string();
            self.script_editor(stage)
                .clone()
                .update(cx, |s, cx| s.set_value(text, window, cx));
        }

        self.auth_kind = AuthKind::of(&self.spec.auth);
        // An empty token starts editable; a stored one starts masked.
        self.token_revealed = matches!(&self.spec.auth, Auth::Bearer { token } if token.is_empty())
            || !matches!(self.spec.auth, Auth::Bearer { .. });
        let (token, user, pass, key, value, in_query) = match &self.spec.auth {
            Auth::Bearer { token } => (token.as_str(), "", "", "", "", false),
            Auth::Basic { username, password } => {
                ("", username.as_str(), password.as_str(), "", "", false)
            }
            Auth::ApiKey {
                key,
                value,
                in_query,
            } => ("", "", "", key.as_str(), value.as_str(), *in_query),
            Auth::None | Auth::Inherit => ("", "", "", "", "", false),
        };
        self.api_in_query = in_query;
        let timeout = self.spec.timeout_ms.map(|v| v.to_string()).unwrap_or_default();
        self.request_timeout.update(cx, |s, cx| s.set_value(timeout, window, cx));
        self.redirect_limit.update(cx, |s, cx| {
            s.set_value(
                self.spec
                    .max_redirects
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
                window,
                cx,
            )
        });
        for (state, text, tokens) in [
            (&self.token, token, true),
            (&self.username, user, true),
            (&self.password, pass, false),
            (&self.api_key, key, false),
            (&self.api_value, value, true),
        ] {
            let content = if tokens {
                var_content(text)
            } else {
                text.to_string().into()
            };
            state.update(cx, |s, cx| s.set_value(content, window, cx));
        }
        cx.notify();
    }

    /// Replaces raw body text. An empty body mode switches to JSON so the text
    /// is visible. Emits `Changed`.
    pub fn set_body_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.spec.body.kind, BodyKind::Json | BodyKind::Text) {
            self.spec.body.kind = BodyKind::Json;
        }
        self.body_editor
            .update(cx, |s, cx| s.set_value(text.clone(), window, cx));
        self.spec.body.text = text;
        self.highlight_body_vars(cx);
        self.body_error = None;
        self.field_hover = None;
        self.refresh_body_diagnostics(cx);
        cx.emit(RequestEditorEvent::Changed);
        cx.notify();
    }

    /// Apply the media type and example generated from the linked operation.
    pub fn set_generated_body(
        &mut self,
        generated: GeneratedRequestBody,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.spec.body = generated.body;
        self.spec
            .headers
            .retain(|header| !header.key.eq_ignore_ascii_case("content-type"));
        if let Some(content_type) = generated.content_type {
            self.spec
                .headers
                .push(KeyValue::new("Content-Type", content_type));
        }
        self.set_spec(self.spec.clone(), window, cx);
        cx.emit(RequestEditorEvent::Changed);
        cx.notify();
    }

    pub fn active_tab(&self) -> RequestEditorTab {
        self.tab
    }

    pub fn script_stage(&self) -> ScriptStage {
        self.script_stage
    }

    pub fn set_script_stage(&mut self, stage: ScriptStage, cx: &mut Context<Self>) {
        self.script_stage = stage;
        cx.notify();
    }

    /// True when either script has content: the Scripts tab shows a dot.
    pub fn has_scripts(&self) -> bool {
        !self.spec.scripts.is_empty()
    }

    fn script_editor(&self, stage: ScriptStage) -> &Entity<EditorState> {
        match stage {
            ScriptStage::PreRequest => &self.pre_script,
            ScriptStage::PostResponse => &self.post_script,
        }
    }

    /// Appends snippet `ix` of the current stage to its script (on a new line).
    /// Emits `Changed`.
    pub fn insert_snippet(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let stage = self.script_stage;
        let Some((_, code)) = stage.snippets().get(ix) else {
            return;
        };
        let current = stage.script(&self.spec);
        let text = if current.trim().is_empty() {
            code.to_string()
        } else if current.ends_with('\n') {
            format!("{current}{code}")
        } else {
            format!("{current}\n{code}")
        };
        self.script_editor(stage)
            .clone()
            .update(cx, |s, cx| s.set_value(text.clone(), window, cx));
        *stage.script_mut(&mut self.spec) = text;
        cx.emit(RequestEditorEvent::Changed);
        cx.notify();
    }

    pub fn set_redirect_defaults(&mut self, follow: bool, max: u32, cx: &mut Context<Self>) {
        self.redirect_defaults = (follow, max);
        cx.notify();
    }

    /// The app's settings the request's own ones fall back to (shown as hints).
    pub fn set_app_defaults(&mut self, settings: &crate::core::types::Settings, cx: &mut Context<Self>) {
        self.redirect_defaults = (settings.follow_redirects, settings.max_redirects);
        self.connection_defaults = (settings.timeout_ms, settings.ssl_verify);
        cx.notify();
    }

    fn set_ssl_verify(&mut self, verify: Option<bool>, cx: &mut Context<Self>) {
        if self.spec.ssl_verify != verify {
            self.spec.ssl_verify = verify;
            cx.emit(RequestEditorEvent::Changed);
        }
        cx.notify();
    }

    fn set_follow_redirects(&mut self, follow: Option<bool>, cx: &mut Context<Self>) {
        if self.spec.follow_redirects != follow {
            self.spec.follow_redirects = follow;
            cx.emit(RequestEditorEvent::Changed);
        }
        cx.notify();
    }

    pub fn set_tab(&mut self, tab: RequestEditorTab, cx: &mut Context<Self>) {
        if tab == RequestEditorTab::Body {
            return;
        }
        self.tab = tab;
        self.auth_menu_open = false;
        cx.notify();
    }

    /// Inline Beautify error ("Невалидный JSON: …"), if the last attempt failed.
    pub fn body_error(&self) -> Option<&str> {
        self.body_error.as_deref()
    }

    pub fn token_revealed(&self) -> bool {
        self.token_revealed
    }

    /// Enabled rows with a non-empty key: the number shown next to a tab label.
    pub fn count(&self, tab: RequestEditorTab) -> usize {
        let list = match tab {
            RequestEditorTab::Params => &self.spec.params,
            RequestEditorTab::Headers => &self.spec.headers,
            _ => return 0,
        };
        list.iter()
            .filter(|kv| kv.enabled && !kv.key.trim().is_empty())
            .count()
    }

    pub fn set_body_kind(&mut self, kind: BodyKind, cx: &mut Context<Self>) {
        if self.spec.body.kind != kind {
            self.spec.body.kind = kind;
            self.body_error = None;
            self.field_hover = None;
            self.refresh_body_diagnostics(cx);
            cx.emit(RequestEditorEvent::Changed);
        }
        cx.notify();
    }

    /// Links the request to a spec operation (`None`: not linked). Drives the field
    /// popovers and the schema warnings of the body.
    pub fn set_schema_link(&mut self, link: Option<SchemaLink>, cx: &mut Context<Self>) {
        if self.schema_link == link {
            return;
        }
        self.schema_link = link;
        self.field_hover = None;
        self.param_hover = None;
        self.refresh_body_diagnostics(cx);
        cx.notify();
    }

    pub fn schema_link(&self) -> Option<&SchemaLink> {
        self.schema_link.as_ref()
    }

    /// The body editor (tests: positions, diagnostics).
    pub fn body_editor(&self) -> &Entity<EditorState> {
        &self.body_editor
    }

    /// Schema info shown for the body field under the pointer.
    pub fn field_hover(&self) -> Option<&FieldInfo> {
        self.field_hover.as_ref().map(|h| &h.info)
    }

    /// Shows the field popover for byte `offset` of the body, as a pointer resting
    /// there would (tests; the editor's hover provider ends up here too).
    pub fn hover_body_offset(&mut self, offset: usize, cx: &mut Context<Self>) {
        let text = self.spec.body.text.clone();
        self.hover_body_text(&text, offset, cx);
    }

    fn hover_body_text(&mut self, text: &str, offset: usize, cx: &mut Context<Self>) {
        let hover = self
            .schema_link
            .as_ref()
            .filter(|_| self.spec.body.kind == BodyKind::Json)
            .and_then(|link| {
                FieldHover::at(link, BodySide::Request, text, offset, &self.body_mismatches)
            });
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

    fn set_param_hover(&mut self, target: KvTarget, ix: usize, hovered: bool, cx: &mut Context<Self>) {
        let next = if hovered {
            Some((target, ix))
        } else if self.param_hover == Some((target, ix)) {
            None
        } else {
            self.param_hover
        };
        if self.param_hover != next {
            self.param_hover = next;
            cx.notify();
        }
    }

    /// Spec info of the param / header in row `ix` of `target`, if linked and declared.
    fn param_info(&self, target: KvTarget, ix: usize) -> Option<FieldInfo> {
        let link = self.schema_link.as_ref()?;
        let key = target.list(&self.spec).get(ix)?.key.trim();
        let (spec, r) = (&link.spec, &link.spec_ref);
        match target {
            KvTarget::Params => schema_hover::param_info(spec, r, key, "query")
                .or_else(|| schema_hover::param_info(spec, r, key, "path")),
            KvTarget::PathVars => schema_hover::param_info(spec, r, key, "path"),
            KvTarget::Headers => schema_hover::param_info(spec, r, key, "header"),
            KvTarget::Form => None,
        }
    }

    /// Live findings of the body check (syntax error, schema warnings).
    pub fn body_problems(&self) -> &[BodyProblem] {
        &self.body_problems
    }

    /// Diagnostics currently set on the body editor: (byte range, severity, message).
    pub fn body_diagnostics(&self, cx: &App) -> Vec<(Range<usize>, DiagnosticSeverity, String)> {
        self.body_editor
            .read(cx)
            .diagnostics()
            .map(|set| {
                set.iter()
                    .map(|d| (d.range.clone(), d.severity, d.message.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Text of the summary line under the body (`None` when hidden).
    pub fn body_summary(&self) -> Option<String> {
        if !self.body_checked {
            return None;
        }
        let errors = self
            .body_problems
            .iter()
            .filter(|p| p.severity == DiagnosticSeverity::Error)
            .count();
        let warnings = self.body_problems.len() - errors;
        Some(if errors > 0 {
            format!("✗ {errors} {}", plural_ru(errors, "ошибка", "ошибки", "ошибок"))
        } else if warnings > 0 {
            format!(
                "⚠ {warnings} {} по схеме",
                plural_ru(warnings, "предупреждение", "предупреждения", "предупреждений")
            )
        } else {
            "✓ Валидный JSON".to_string()
        })
    }

    /// Re-checks a JSON body: syntax (with `{{var}}` masked) and, when linked, the
    /// request schema. Findings become editor diagnostics (squiggle + hover message).
    fn refresh_body_diagnostics(&mut self, cx: &mut Context<Self>) {
        let text = &self.spec.body.text;
        let checked = self.spec.body.kind == BodyKind::Json
            && !text.trim().is_empty()
            && text.len() <= LIVE_CHECK_LIMIT;
        let mut problems = Vec::new();
        let mut mismatches = Vec::new();
        if checked {
            match schema_hover::check_json(text) {
                Err(e) => problems.push(BodyProblem {
                    range: e.range,
                    severity: DiagnosticSeverity::Error,
                    message: e.message,
                }),
                Ok(Some(value)) => {
                    if let Some(found) = self.schema_link.as_ref().and_then(|link| {
                        schema_hover::validate_request_body(&link.spec, &link.spec_ref, &value)
                    }) {
                        for m in &found {
                            let range = schema_hover::parse_path(&m.path)
                                .and_then(|path| schema_hover::json_path_range(text, &path))
                                .unwrap_or(0..text.len().min(1));
                            problems.push(BodyProblem {
                                range,
                                severity: DiagnosticSeverity::Warning,
                                message: format!(
                                    "{}: ожидается {}, получено {}",
                                    m.title, m.expected, m.actual
                                ),
                            });
                        }
                        mismatches = found;
                    }
                }
                Ok(None) => {}
            }
        }
        problems.sort_by_key(|p| p.range.start);
        self.body_checked = checked;
        self.body_mismatches = mismatches;
        self.body_problems = problems.clone();
        self.body_editor.update(cx, |s, cx| {
            let rope = s.text().clone();
            if let Some(set) = s.diagnostics_mut() {
                set.reset(&rope);
                for p in problems {
                    let start = rope.offset_to_position(p.range.start);
                    let end = rope.offset_to_position(p.range.end);
                    set.push(Diagnostic::new(start..end, p.message).with_severity(p.severity));
                }
            }
            cx.notify();
        });
    }

    /// Summary click: caret to the first problem, editor focused.
    fn reveal_first_problem(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(start) = self.body_problems.first().map(|p| p.range.start) else {
            return;
        };
        self.body_editor.update(cx, |s, cx| {
            s.set_selected_range(start..start, cx);
            s.focus(window, cx);
        });
    }

    /// Byte ranges of `{{var}}` currently highlighted in the body editor.
    pub fn body_var_ranges(&self, cx: &App) -> Vec<std::ops::Range<usize>> {
        self.body_vars
            .as_ref()
            .map(|(text, _, _)| text.get_ranges(cx))
            .unwrap_or_default()
    }

    /// Re-marks every `{{var}}` in the body, like the inline tokens in the URL
    /// field. The body stays plain text (a JSON editor can't hold atomic tokens).
    fn highlight_body_vars(&mut self, cx: &mut Context<Self>) {
        let p = *cx.palette();
        let ranges = crate::ui::var_ranges(&self.spec.body.text);
        let text_decorations: Vec<_> = ranges
            .iter()
            .map(|r| {
                TextDecoration::new(
                    r.clone(),
                    HighlightStyle {
                        color: Some(p.accent_hi),
                        font_weight: Some(FontWeight::MEDIUM),
                        ..Default::default()
                    },
                )
            })
            .collect();
        let fills: Vec<_> = ranges
            .into_iter()
            .map(|r| {
                RangeDecoration::new(r)
                    .with_style(RangeDecorationStyle::Fill)
                    .with_color(p.accent.opacity(0.18))
            })
            .collect();
        match &mut self.body_vars {
            Some((text, fill, mode)) => {
                text.set(text_decorations, cx);
                fill.set(fills, cx);
                *mode = p.mode;
            }
            None => {
                let (text, fill) = self.body_editor.update(cx, |s, cx| {
                    (
                        s.create_decorations_collection(text_decorations, cx),
                        s.create_range_decorations_collection(fills, cx),
                    )
                });
                self.body_vars = Some((text, fill, p.mode));
            }
        }
    }

    /// Reformats a JSON body; on a parse error shows it inline and leaves the text.
    pub fn beautify(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match beautify_json(&self.spec.body.text) {
            Ok(pretty) => {
                self.body_error = None;
                if pretty != self.spec.body.text {
                    self.body_editor
                        .update(cx, |s, cx| s.set_value(pretty.clone(), window, cx));
                    self.spec.body.text = pretty;
                    self.highlight_body_vars(cx);
                    self.field_hover = None;
                    self.refresh_body_diagnostics(cx);
                    cx.emit(RequestEditorEvent::Changed);
                }
            }
            Err(e) => self.body_error = Some(format!("Невалидный JSON: {e}")),
        }
        cx.notify();
    }

    pub fn set_auth_kind(&mut self, kind: AuthKind, cx: &mut Context<Self>) {
        if kind == AuthKind::Bearer
            && self.auth_kind != kind
            && self.token.read(cx).value().is_empty()
        {
            self.token_revealed = true;
        }
        self.auth_kind = kind;
        self.auth_menu_open = false;
        self.sync_auth(cx);
    }

    pub fn set_api_key_in_query(&mut self, in_query: bool, cx: &mut Context<Self>) {
        self.api_in_query = in_query;
        self.sync_auth(cx);
    }

    pub fn toggle_token_revealed(&mut self, cx: &mut Context<Self>) {
        self.token_revealed = !self.token_revealed;
        cx.notify();
    }

    /// Rebuilds `spec.auth` from the picked type and the auth inputs.
    fn sync_auth(&mut self, cx: &mut Context<Self>) {
        let v = |s: &Entity<InputState>, cx: &Context<Self>| s.read(cx).value().to_string();
        let auth = match self.auth_kind {
            AuthKind::None => Auth::None,
            AuthKind::Inherit => Auth::Inherit,
            AuthKind::Bearer => Auth::Bearer {
                token: v(&self.token, cx),
            },
            AuthKind::Basic => Auth::Basic {
                username: v(&self.username, cx),
                password: v(&self.password, cx),
            },
            AuthKind::ApiKey => Auth::ApiKey {
                key: v(&self.api_key, cx),
                value: v(&self.api_value, cx),
                in_query: self.api_in_query,
            },
        };
        if auth != self.spec.auth {
            self.spec.auth = auth;
            cx.emit(RequestEditorEvent::Changed);
        }
        cx.notify();
    }

    /// Path Variables rows for the current list (no draft row: keys come from the URL).
    fn rebuild_path_var_rows(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let list = self.spec.path_vars.clone();
        self.path_vars = list
            .iter()
            .map(|kv| self.make_row(KvTarget::PathVars, Some(kv), window, cx))
            .collect();
    }

    /// The URL changed in the request line: add/remove Path Variables rows to
    /// match its `:name` segments, keeping the values already typed.
    pub fn set_url(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.spec.url = url.to_string();
        let synced = vars::sync_path_vars(url, &self.spec.path_vars);
        if synced != self.spec.path_vars {
            self.spec.path_vars = synced;
            self.rebuild_path_var_rows(window, cx);
            cx.emit(RequestEditorEvent::Changed);
            cx.notify();
        }
    }

    /// Whether `tab` (Params / Headers / Body form) shows its rows as `key:value` text.
    pub fn is_bulk_edit(&self, tab: RequestEditorTab) -> bool {
        KvTarget::of_tab(tab).is_some_and(|t| self.bulk.iter().any(|b| b.target == t))
    }

    /// Current bulk text of `tab`, when it is in bulk mode.
    pub fn bulk_text(&self, tab: RequestEditorTab, cx: &App) -> Option<String> {
        let target = KvTarget::of_tab(tab)?;
        let bulk = self.bulk.iter().find(|b| b.target == target)?;
        Some(bulk.editor.read(cx).value().to_string())
    }

    /// Switches the Params / Headers / Body form table of `tab` between rows
    /// and bulk text ("Bulk Edit" / "Key-Value Edit" in the table header).
    /// Rows → text → rows keeps order, enabled flags and descriptions.
    pub fn toggle_bulk_edit(&mut self, tab: RequestEditorTab, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = KvTarget::of_tab(tab) else {
            return;
        };
        self.expanded = None;
        if let Some(pos) = self.bulk.iter().position(|b| b.target == target) {
            self.bulk.remove(pos);
            let rows: Vec<RowInputs> = target
                .list(&self.spec)
                .clone()
                .iter()
                .map(Some)
                .chain([None])
                .map(|kv| self.make_row(target, kv, window, cx))
                .collect();
            *self.rows_mut(target) = rows;
            self.param_hover = None;
            cx.notify();
            return;
        }
        let original = target.list(&self.spec).clone();
        let text = bulk::to_text(&original);
        let editor = cx.new(|cx| {
            let mut s = EditorState::new(window, cx);
            s.set_value(text, window, cx);
            s
        });
        let subscription = cx.subscribe_in(
            &editor,
            window,
            move |this, state, ev: &InputEvent, _, cx| {
                if !matches!(ev, InputEvent::Change) {
                    return;
                }
                let text = state.read(cx).value().to_string();
                // `set_spec` reloading the text is not an edit.
                if text == bulk::to_text(target.list(&this.spec)) {
                    return;
                }
                let Some(original) =
                    this.bulk.iter().find(|b| b.target == target).map(|b| &b.original)
                else {
                    return;
                };
                let rows = bulk::from_text(&text, original);
                *target.list_mut(&mut this.spec) = rows;
                cx.emit(RequestEditorEvent::Changed);
                cx.notify();
            },
        );
        editor.update(cx, |s, cx| s.focus(window, cx));
        self.bulk.push(BulkEdit { target, editor, original, _subscription: subscription });
        cx.notify();
    }

    /// Problems of header row `ix` as typed: `(name, value)`. `{{vars}}` are
    /// checked as placeholders here; their values are checked when sending.
    pub fn header_issues(&self, ix: usize) -> (Option<HeaderIssue>, Option<HeaderIssue>) {
        let Some(h) = self.spec.headers.get(ix) else {
            return (None, None);
        };
        let name = vars::mask_placeholders(h.key.trim(), "x");
        (
            header_check::check_name(&name, !h.value.is_empty()),
            header_check::check_value(&h.value),
        )
    }

    fn rows(&self, target: KvTarget) -> &Vec<RowInputs> {
        match target {
            KvTarget::Params => &self.params,
            KvTarget::Headers => &self.headers,
            KvTarget::Form => &self.form,
            KvTarget::PathVars => &self.path_vars,
        }
    }

    fn rows_mut(&mut self, target: KvTarget) -> &mut Vec<RowInputs> {
        match target {
            KvTarget::Params => &mut self.params,
            KvTarget::Headers => &mut self.headers,
            KvTarget::Form => &mut self.form,
            KvTarget::PathVars => &mut self.path_vars,
        }
    }

    fn make_row(
        &mut self,
        target: KvTarget,
        kv: Option<&KeyValue>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> RowInputs {
        let id = self.next_row_id;
        self.next_row_id += 1;
        let placeholder = match target {
            KvTarget::Headers => "Header",
            KvTarget::Params => "Param",
            KvTarget::Form => "Field",
            KvTarget::PathVars => "Variable",
        };
        let key = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder(placeholder);
            if let Some(kv) = kv {
                s.set_value(kv.key.clone(), window, cx);
            }
            s
        });
        let value = cx.new(|cx| {
            let placeholder = if target == KvTarget::Form && kv.is_some_and(|field| field.file) {
                "Путь к файлу"
            } else {
                "Value"
            };
            let mut s = InputState::new(window, cx).placeholder(placeholder);
            if let Some(kv) = kv {
                s.set_value(var_content(&kv.value), window, cx);
            }
            s
        });
        let description = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("Description");
            if let Some(kv) = kv {
                s.set_value(kv.description.clone(), window, cx);
            }
            s
        });
        let subscriptions = [
            (&key, Field::Key),
            (&value, Field::Value),
            (&description, Field::Description),
        ]
        .into_iter()
        .map(|(state, field)| {
            cx.subscribe_in(
                state,
                window,
                move |this, state, ev: &InputEvent, window, cx| {
                    if let InputEvent::Change = ev {
                        this.on_cell_change(target, id, field, state, window, cx);
                    }
                },
            )
        })
        .collect();
        RowInputs {
            id,
            key,
            value,
            description,
            _subscriptions: subscriptions,
        }
    }

    fn on_cell_change(
        &mut self,
        target: KvTarget,
        row_id: u64,
        field: Field,
        state: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if field == Field::Value {
            state.update(cx, |s, cx| tokenize_vars(s, window, cx));
        }
        let text = state.read(cx).value().to_string();
        let Some(pos) = self.rows(target).iter().position(|r| r.id == row_id) else {
            return;
        };
        let len = target.list(&self.spec).len();
        if pos >= len {
            // The draft row: becomes real once it has content.
            if text.is_empty() {
                return;
            }
            target.list_mut(&mut self.spec).push(KeyValue::new("", ""));
            let draft = self.make_row(target, None, window, cx);
            self.rows_mut(target).push(draft);
        }
        let kv = &mut target.list_mut(&mut self.spec)[pos];
        let slot = match field {
            Field::Key => &mut kv.key,
            Field::Value => &mut kv.value,
            Field::Description => &mut kv.description,
        };
        if *slot != text {
            *slot = text;
            cx.emit(RequestEditorEvent::Changed);
        }
        cx.notify();
    }

    /// Whether a value cell is open as the multi-line editor (tests).
    pub fn has_expanded_value(&self) -> bool {
        self.expanded.is_some()
    }

    /// Expands the focused value cell / folds the expanded one once it loses
    /// focus. Checked while rendering: focus events need an active window.
    fn sync_expanded_value(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(expanded) = &self.expanded {
            if !expanded.state.read(cx).focus_handle(cx).is_focused(window) {
                cx.defer_in(window, |this, window, cx| {
                    let lost = this
                        .expanded
                        .as_ref()
                        .is_some_and(|e| !e.state.read(cx).focus_handle(cx).is_focused(window));
                    if lost {
                        this.collapse_value(window, cx);
                    }
                });
            }
            return;
        }
        let target = match self.tab {
            RequestEditorTab::Params => KvTarget::Params,
            RequestEditorTab::Headers => KvTarget::Headers,
            _ => KvTarget::Form,
        };
        let focused = self
            .rows(target)
            .iter()
            .find(|r| r.value.read(cx).focus_handle(cx).is_focused(window))
            .map(|r| r.id);
        if let Some(row_id) = focused {
            cx.defer_in(window, move |this, window, cx| this.expand_value(target, row_id, window, cx));
        }
    }

    /// Opens value cell `row_id` as a wrapping editor when its text doesn't fit.
    fn expand_value(&mut self, target: KvTarget, row_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.expanded.is_some() {
            return;
        }
        let Some(row) = self.rows(target).iter().find(|r| r.id == row_id) else {
            return;
        };
        let text = row.value.read(cx).value().to_string();
        let width = self.value_widths.borrow().get(&(target, row_id)).copied();
        let Some(width) = width else { return };
        if mono_text_width(&text) <= width - 12. {
            return;
        }
        let state = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx).auto_grow(1, 8);
            s.set_value(text, window, cx);
            s
        });
        let subscription = cx.subscribe_in(
            &state,
            window,
            move |this, state, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => {
                    let text = state.read(cx).value().to_string();
                    // A header/param value is one line: Enter finishes editing.
                    if text.contains('\n') {
                        let flat = text.replace(['\r', '\n'], "");
                        state.update(cx, |s, cx| s.set_value(flat, window, cx));
                        this.collapse_value(window, cx);
                        return;
                    }
                    let Some(pos) = this.rows(target).iter().position(|r| r.id == row_id) else {
                        return;
                    };
                    if let Some(kv) = target.list_mut(&mut this.spec).get_mut(pos) {
                        if kv.value != text {
                            kv.value = text;
                            cx.emit(RequestEditorEvent::Changed);
                        }
                    }
                    cx.notify();
                }
                InputEvent::Blur => this.collapse_value(window, cx),
                _ => {}
            },
        );
        state.update(cx, |s, cx| s.focus(window, cx));
        self.expanded = Some(ExpandedValue { target, row_id, state, _subscription: subscription });
        cx.notify();
    }

    /// Back to the one-line cell, showing the edited value.
    fn collapse_value(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(expanded) = self.expanded.take() else {
            return;
        };
        let text = expanded.state.read(cx).value().to_string();
        if let Some(row) = self.rows(expanded.target).iter().find(|r| r.id == expanded.row_id) {
            row.value.clone().update(cx, |s, cx| s.set_value(var_content(&text), window, cx));
        }
        cx.notify();
    }

    fn toggle_row(&mut self, target: KvTarget, ix: usize, cx: &mut Context<Self>) {
        if let Some(kv) = target.list_mut(&mut self.spec).get_mut(ix) {
            kv.enabled = !kv.enabled;
            cx.emit(RequestEditorEvent::Changed);
            cx.notify();
        }
    }

    fn delete_row(&mut self, target: KvTarget, ix: usize, cx: &mut Context<Self>) {
        if ix < target.list(&self.spec).len() {
            self.expanded = None;
            target.list_mut(&mut self.spec).remove(ix);
            self.rows_mut(target).remove(ix);
            cx.emit(RequestEditorEvent::Changed);
            cx.notify();
        }
    }

    fn set_form_field_type(&mut self, ix: usize, file: bool, cx: &mut Context<Self>) {
        if let Some(field) = self.spec.body.form.get_mut(ix) {
            if field.file != file {
                field.file = file;
                cx.emit(RequestEditorEvent::Changed);
                cx.notify();
            }
        } else if file {
            // The draft row becomes a file field only after a file is selected.
            self.pick_file(FileTarget::NewFormField, cx);
        }
    }

    fn pick_file(&mut self, target: FileTarget, cx: &mut Context<Self>) {
        let request_id = self.spec.id.clone();
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Выберите файл для тела запроса".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                if let Some(path) = paths.into_iter().next() {
                    this.update_in(cx, |this, window, cx| {
                        if this.spec.id != request_id {
                            return;
                        }
                        let path_text = path.display().to_string();
                        match target {
                            FileTarget::NewFormField
                                if this.spec.body.kind == BodyKind::Multipart =>
                            {
                                let mut field = KeyValue::new("file", path_text);
                                field.file = true;
                                let row = this.make_row(KvTarget::Form, Some(&field), window, cx);
                                this.spec.body.form.push(field);
                                let draft = this.form.len().saturating_sub(1);
                                this.form.insert(draft, row);
                            }
                            FileTarget::FormField(row_id)
                                if this.spec.body.kind == BodyKind::Multipart =>
                            {
                                let Some(ix) = this.form.iter().position(|row| row.id == row_id)
                                else {
                                    return;
                                };
                                let Some(field) = this.spec.body.form.get_mut(ix) else {
                                    return;
                                };
                                field.file = true;
                                field.value = path_text.clone();
                                this.form[ix]
                                    .value
                                    .update(cx, |state, cx| state.set_value(path_text, window, cx));
                            }
                            FileTarget::BinaryBody if this.spec.body.kind == BodyKind::Binary => {
                                this.spec.body.file_path = Some(path);
                            }
                            _ => return,
                        }
                        cx.emit(RequestEditorEvent::Changed);
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    // ----- rendering -----

    fn move_split(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.split_dragging {
            return;
        }
        if event.pressed_button.is_none() {
            self.stop_split(cx);
            return;
        }
        let Some(bounds) = self.split_bounds.get() else {
            return;
        };
        let available = bounds.size.height.as_f32();
        let usable = available - PANE_SPLITTER_HEIGHT;
        if usable <= 0. {
            return;
        }
        let cursor_y = event.position.y.as_f32() - bounds.origin.y.as_f32();
        let top = details_height(available, (cursor_y - PANE_SPLITTER_HEIGHT / 2.) / usable);
        let ratio = top / usable;
        if (self.split_ratio - ratio).abs() > f32::EPSILON {
            self.split_ratio = ratio;
            cx.notify();
        }
    }

    fn stop_split(&mut self, cx: &mut Context<Self>) {
        if self.split_dragging {
            self.split_dragging = false;
            cx.notify();
        }
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *cx.palette();
        let tabs = [
            RequestEditorTab::Params,
            RequestEditorTab::Headers,
            RequestEditorTab::Auth,
            RequestEditorTab::Scripts,
            RequestEditorTab::Docs,
            RequestEditorTab::Settings,
        ]
        .into_iter()
        .map(|tab| {
            let active = tab == self.tab;
            let count = self.count(tab);
            let dot = (tab == RequestEditorTab::Scripts && self.has_scripts())
                || (tab == RequestEditorTab::Docs && !self.spec.description.trim().is_empty());
            div()
                .id(("subtab", tab as usize))
                .flex()
                .items_center()
                .gap(px(5.))
                .h(px(32.))
                .px(px(2.))
                .border_b_2()
                .border_color(if active {
                    p.accent
                } else {
                    transparent_black()
                })
                .text_size(px(12.))
                .cursor_pointer()
                .text_color(if active { p.text } else { p.text_dim })
                .hover(|s| s.text_color(p.text))
                .on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx)))
                .child(tab.label())
                .when(count > 0, |d| {
                    d.child(
                        div()
                            .text_size(px(10.5))
                            .font_family(MONO)
                            .text_color(if active { p.accent_hi } else { p.text_dim })
                            .child(count.to_string()),
                    )
                })
                .when(dot, |d| {
                    d.child(
                        div()
                            .id(if tab == RequestEditorTab::Docs { "docs-dot" } else { "scripts-dot" })
                            .size(px(6.))
                            .rounded_full()
                            .bg(if active { p.accent_hi } else { p.accent })
                            .test_support(),
                    )
                })
                .test_support()
        });

        div()
            .flex()
            .flex_wrap()
            .flex_none()
            .items_center()
            .gap_x(px(16.))
            .gap_y(px(4.))
            .min_h(px(38.))
            .py(px(2.))
            .px(px(14.))
            .border_b_1()
            .border_color(p.border)
            .children(tabs)
            .child(div().flex_1())
            .when_some(self.spec.spec_ref.as_ref(), |d, r| {
                d.child(
                    Pill::new(format!("Linked to spec · {}", r.method.full_label()))
                        .color(p.accent),
                )
            })
            .child(
                Button::new("request-save", "Save")
                    .ghost()
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(RequestEditorEvent::Save))),
            )
    }

    /// "Bulk Edit" / "Key-Value Edit" cell at the right of a table header (as in Postman).
    fn bulk_toggle(&self, target: KvTarget, cx: &mut Context<Self>) -> AnyElement {
        let p = *cx.palette();
        let bulk = self.bulk.iter().any(|b| b.target == target);
        let tab = match target {
            KvTarget::Params => RequestEditorTab::Params,
            KvTarget::Headers => RequestEditorTab::Headers,
            KvTarget::Form => RequestEditorTab::Body,
            KvTarget::PathVars => return div().into_any_element(),
        };
        div()
            .id("bulk-toggle")
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .flex()
            .items_center()
            .px(px(12.))
            .border_l_1()
            .border_color(p.border)
            .bg(p.bg)
            .text_size(px(11.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(p.text)
            .cursor_pointer()
            .hover(|s| s.bg(p.surface_hi))
            .on_click(cx.listener(move |this, _, window, cx| this.toggle_bulk_edit(tab, window, cx)))
            .child(if bulk { "Key-Value Edit" } else { "Bulk Edit" })
            .test_support()
            .into_any_element()
    }

    fn render_kv(&self, target: KvTarget, cx: &mut Context<Self>) -> AnyElement {
        let p = *cx.palette();
        if let Some(bulk) = self.bulk.iter().find(|b| b.target == target) {
            return div()
                .id(target.scope())
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .relative()
                        .flex_none()
                        .h(px(30.))
                        .border_b_1()
                        .border_color(p.border)
                        .child(self.bulk_toggle(target, cx)),
                )
                .child(
                    div()
                        .id("bulk-editor")
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .child(
                            Editor::new(&bulk.editor)
                                .appearance(false)
                                .bordered(false)
                                .font_family(MONO)
                                .text_size(px(12.))
                                .h(relative(1.)),
                        )
                        .test_support(),
                )
                .child(
                    div()
                        .flex_none()
                        .px(px(14.))
                        .py(px(5.))
                        .border_t_1()
                        .border_color(p.border)
                        .text_size(px(11.))
                        .text_color(p.text_dim)
                        .child("key:value — по одной на строку; // в начале строки выключает её"),
                )
                .into_any_element();
        }
        let list = target.list(&self.spec);
        let is_multipart = target == KvTarget::Form && self.spec.body.kind == BodyKind::Multipart;
        let path_vars = target == KvTarget::PathVars;
        let header = div()
            .relative()
            .flex()
            .items_center()
            .h(px(30.))
            .px(px(14.))
            .gap(px(8.))
            .border_b_1()
            .border_color(p.border)
            .text_size(px(10.5))
            .text_color(p.text_dim)
            .child(div().w(px(16.)).flex_none())
            .child(div().flex_1().child(if path_vars { "PATH VARIABLE" } else { "KEY" }))
            .when(is_multipart, |d| {
                d.child(div().w(px(84.)).flex_none().child("TYPE"))
            })
            .child(
                div()
                    .flex_basis(relative(0.))
                    .flex_grow(1.4)
                    .flex_shrink(1.)
                    .child("VALUE"),
            )
            .child(div().flex_1().child("DESCRIPTION"))
            .child(div().w(px(22.)).flex_none())
            .child(self.bulk_toggle(target, cx));

        let issue_badge = |id: (&'static str, usize), issue: &HeaderIssue| {
            let color = if issue.is_error() { p.danger } else { p.warn };
            div()
                .id(id)
                .flex_none()
                .px(px(4.))
                .child(icon(IconName::TriangleAlert, 12., color))
                .tooltip(Tooltip::new(issue.message.clone()).build())
                .test_support()
        };
        let issue_border = |d: Stateful<Div>, issue: &Option<HeaderIssue>| match issue {
            Some(i) => d
                .rounded(px(4.))
                .border_1()
                .border_color(if i.is_error() { p.danger } else { p.warn }),
            None => d,
        };
        let rows = self.rows(target).iter().enumerate().map(|(ix, row)| {
            let kv = list.get(ix);
            let (name_issue, value_issue) = if target == KvTarget::Headers && kv.is_some() {
                self.header_issues(ix)
            } else {
                (None, None)
            };
            let row_id = row.id;
            let draft = kv.is_none();
            let enabled = kv.is_none_or(|kv| kv.enabled);
            let cell = |state: &Entity<InputState>, id: &'static str, mono: bool| {
                Input::new(state)
                    .id((id, ix))
                    .appearance(false)
                    .text_size(px(12.))
                    .h(px(30.))
                    .when(mono, |i| i.font_family(MONO))
            };
            let toggle = div()
                .id(("enabled", ix))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(16.))
                .rounded(px(4.))
                .border_1()
                .when(draft || path_vars, |d| d.invisible())
                .when(!draft && !path_vars, |d| {
                    d.cursor_pointer().on_click(
                        cx.listener(move |this, _, _, cx| this.toggle_row(target, ix, cx)),
                    )
                })
                .map(|d| {
                    if enabled {
                        d.bg(p.accent).border_color(p.accent).child(icon(
                            IconName::Check,
                            11.,
                            white(),
                        ))
                    } else {
                        d.border_color(p.border)
                    }
                })
                .test_support();
            let delete = div().w(px(22.)).flex_none().when(!draft && !path_vars, |d| {
                d.child(
                    IconButton::new(("delete", ix), IconName::X)
                        .size(12.)
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.delete_row(target, ix, cx)),
                        ),
                )
            });
            div()
                .id(("row", ix))
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(14.))
                .border_b_1()
                .border_color(p.border.opacity(0.6))
                .child(toggle)
                .child(
                    issue_border(div().id(("key-cell", ix)), &name_issue)
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .when(!enabled, |d| d.opacity(0.45))
                        .when(target != KvTarget::Form && !draft, |d| {
                            d.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                this.set_param_hover(target, ix, *hovered, cx)
                            }))
                        })
                        .map(|d| {
                            if path_vars {
                                // The name comes from the URL (`:name`); edit it there.
                                d.child(
                                    div()
                                        .h(px(30.))
                                        .flex()
                                        .items_center()
                                        .px(px(2.))
                                        .font_family(MONO)
                                        .text_size(px(12.))
                                        .child(kv.map(|kv| kv.key.clone()).unwrap_or_default()),
                                )
                            } else {
                                d.child(cell(&row.key, "key", true))
                            }
                        })
                        .when_some(name_issue.as_ref(), |d, issue| {
                            d.child(
                                div()
                                    .absolute()
                                    .right(px(2.))
                                    .top(px(8.))
                                    .child(issue_badge(("key-issue", ix), issue)),
                            )
                        })
                        .when_some(
                            (self.param_hover == Some((target, ix)))
                                .then(|| self.param_info(target, ix))
                                .flatten(),
                            |d, info| {
                                d.child(
                                    deferred(
                                        anchored()
                                            .snap_to_window_with_margin(px(8.))
                                            .child(
                                                div()
                                                    .mt(px(2.))
                                                    .child(field_hover_card(&info, None, &p)),
                                            ),
                                    )
                                    .with_priority(2),
                                )
                            },
                        )
                        .test_support(),
                )
                .when(is_multipart, |d| {
                    let view = cx.entity().downgrade();
                    d.child(FormFieldTypePicker::new(
                        ("form-type", ix),
                        kv.is_some_and(|field| field.file),
                        move |file, _, cx| {
                            view.update(cx, |this, cx| this.set_form_field_type(ix, file, cx))
                                .ok();
                        },
                    ))
                })
                .child(
                    div()
                        .relative()
                        .flex_basis(relative(0.))
                        .flex_grow(1.4)
                        .flex_shrink(1.)
                        .min_w_0()
                        .when(!enabled, |d| d.opacity(0.45))
                        .child({
                            let widths = self.value_widths.clone();
                            canvas(
                                move |bounds, _, _| {
                                    widths
                                        .borrow_mut()
                                        .insert((target, row_id), bounds.size.width.as_f32());
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .inset_0()
                        })
                        .when_some(
                            self.expanded
                                .as_ref()
                                .filter(|e| e.target == target && e.row_id == row_id),
                            |d, expanded| {
                                let width = self
                                    .value_widths
                                    .borrow()
                                    .get(&(target, row_id))
                                    .copied()
                                    .unwrap_or(240.);
                                d.child(
                                    div().absolute().top_0().left_0().child(
                                        deferred(
                                            anchored().snap_to_window_with_margin(px(8.)).child(
                                                div()
                                                    .id("value-expanded")
                                                    .w(px(width))
                                                    .max_h(px(200.))
                                                    .overflow_y_scroll()
                                                    .px(px(4.))
                                                    .py(px(2.))
                                                    .rounded(px(6.))
                                                    .bg(p.surface)
                                                    .border_1()
                                                    .border_color(p.accent)
                                                    .shadow_lg()
                                                    .child(
                                                        Textarea::new(&expanded.state)
                                                            .appearance(false)
                                                            .font_family(MONO)
                                                            .text_size(px(12.)),
                                                    )
                                                    .test_support(),
                                            ),
                                        )
                                        .with_priority(3),
                                    ),
                                )
                            },
                        )
                        .child(
                            issue_border(div().id(("value-cell", ix)), &value_issue)
                                .flex()
                                .items_center()
                                .min_w_0()
                                .child(
                                    cell(&row.value, "value", true)
                                        .token(|ctx, _, cx| var_token_element(ctx, cx)),
                                )
                                .when_some(value_issue.as_ref(), |d, issue| {
                                    d.child(issue_badge(("value-issue", ix), issue))
                                })
                                .test_support()
                                .when(is_multipart && kv.is_some_and(|field| field.file), |d| {
                                    d.child(
                                        Button::new(
                                            ("form-pick-file", ix),
                                            if kv.is_some_and(|field| field.value.is_empty()) {
                                                "Выбрать…"
                                            } else {
                                                "Заменить…"
                                            },
                                        )
                                        .ghost()
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.pick_file(FileTarget::FormField(row_id), cx)
                                            }),
                                        ),
                                    )
                                }),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .when(!enabled, |d| d.opacity(0.45))
                        .child(cell(&row.description, "desc", false)),
                )
                .child(delete)
        });

        div()
            .id(target.scope())
            .flex()
            .flex_col()
            // Next to Path Variables the query table takes its natural height and
            // the Params tab scrolls as a whole.
            .map(|d| {
                let natural = path_vars
                    || (target == KvTarget::Params && !self.spec.path_vars.is_empty());
                if natural { d.flex_none() } else { d.flex_1().min_h_0().overflow_y_scroll() }
            })
            .child(header)
            .children(rows)
            .into_any_element()
    }

    /// Params tab: the query table, plus Path Variables when the URL has `:name` segments.
    fn render_params(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.spec.path_vars.is_empty() {
            return self.render_kv(KvTarget::Params, cx);
        }
        let p = *cx.palette();
        let bulk = self.is_bulk_edit(RequestEditorTab::Params);
        div()
            .id("params-with-path")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            // Bulk text fills the space itself; rows scroll together with Path Variables.
            .when(!bulk, |d| d.overflow_y_scroll())
            .child(self.render_kv(KvTarget::Params, cx))
            .child(
                div()
                    .px(px(14.))
                    .pt(px(10.))
                    .pb(px(4.))
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.text_dim)
                    .child("Path Variables"),
            )
            .child(self.render_kv(KvTarget::PathVars, cx))
            .into_any_element()
    }

    fn render_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *cx.palette();
        let kind = self.spec.body.kind;
        let view = cx.entity().downgrade();
        let selected = BODY_KINDS.iter().position(|k| *k == kind).unwrap_or(0);
        let toolbar = div()
            .id("body-toolbar")
            .flex()
            .flex_wrap()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .px(px(14.))
            .py(px(8.))
            .child(BodyModePicker::new(selected, move |ix, _, cx| {
                view.update(cx, |this, cx| this.set_body_kind(BODY_KINDS[ix], cx))
                    .ok();
            }))
            .when_some(self.body_error.clone(), |d, e| {
                d.child(
                    div()
                        .id("body-error")
                        .min_w_0()
                        .overflow_hidden()
                        .child(Pill::new(e).color(p.danger))
                        .test_support(),
                )
            })
            .child(div().flex_1())
            .when(kind == BodyKind::Json, |d| {
                d.child(
                    Button::new("beautify", "Beautify")
                        .ghost()
                        .on_click(cx.listener(|this, _, window, cx| this.beautify(window, cx))),
                )
            })
            .when(self.spec.spec_ref.is_some(), |d| {
                d.child(
                    Button::new("generate-body", "✦ Generate from schema").on_click(
                        cx.listener(|_, _, _, cx| cx.emit(RequestEditorEvent::GenerateBody)),
                    ),
                )
            });

        let content = match kind {
            BodyKind::None => div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_size(px(12.))
                .text_color(p.text_dim)
                .child("У этого запроса нет тела")
                .into_any_element(),
            BodyKind::Json | BodyKind::Text => div()
                .id("body-editor")
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .border_t_1()
                .border_color(p.border)
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    if !*hovered {
                        this.clear_field_hover(cx);
                    }
                }))
                .child(
                    div().flex_1().min_h_0().child(
                        Editor::new(&self.body_editor)
                            .appearance(false)
                            .bordered(false)
                            .font_family(MONO)
                            .text_size(px(12.))
                            .h(relative(1.)),
                    ),
                )
                .when_some(self.body_summary(), |d, summary| {
                    let errors = self
                        .body_problems
                        .iter()
                        .any(|p| p.severity == DiagnosticSeverity::Error);
                    let color = if errors {
                        p.danger
                    } else if self.body_problems.is_empty() {
                        p.online.opacity(0.85)
                    } else {
                        p.warn
                    };
                    let clickable = !self.body_problems.is_empty();
                    d.child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .h(px(24.))
                            .px(px(14.))
                            .border_t_1()
                            .border_color(p.border.opacity(0.6))
                            .child(
                                div()
                                    .id("body-validation")
                                    .text_size(px(11.))
                                    .text_color(color)
                                    .aria_label(summary.clone())
                                    .when(clickable, |d| {
                                        d.cursor_pointer().hover(|s| s.underline()).on_click(
                                            cx.listener(|this, _, window, cx| {
                                                this.reveal_first_problem(window, cx)
                                            }),
                                        )
                                    })
                                    .child(summary)
                                    .test_support(),
                            ),
                    )
                })
                .children(
                    self.field_hover
                        .as_ref()
                        .and_then(|h| field_hover_overlay(h, &self.body_editor, cx)),
                )
                .test_support()
                .into_any_element(),
            BodyKind::Form => self.render_kv(KvTarget::Form, cx).into_any_element(),
            BodyKind::Multipart => div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(
                    div().flex().px(px(14.)).py(px(7.)).child(
                        Button::new("form-add-file", "Добавить файл")
                            .icon(IconName::FolderOpen)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pick_file(FileTarget::NewFormField, cx)
                            })),
                    ),
                )
                .child(self.render_kv(KvTarget::Form, cx))
                .into_any_element(),
            BodyKind::Binary => div()
                .id("binary-body")
                .flex()
                .flex_col()
                .flex_1()
                .gap(px(12.))
                .p(px(16.))
                .child(
                    self.spec
                        .body
                        .file_path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "Файл не выбран".into()),
                )
                .child(Button::new("binary-pick-file", "Выбрать файл…").on_click(
                    cx.listener(|this, _, _, cx| this.pick_file(FileTarget::BinaryBody, cx)),
                ))
                .into_any_element(),
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(toolbar)
            .child(content)
    }

    fn render_scripts(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *cx.palette();
        let current = self.script_stage;
        let stage_picker = div()
            .id("script-stage")
            .flex()
            .flex_none()
            .self_start()
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(7.))
            .bg(p.surface_hi)
            .children(ScriptStage::ALL.into_iter().enumerate().map(|(ix, stage)| {
                let active = stage == current;
                let filled = !stage.script(&self.spec).trim().is_empty();
                div()
                    .id(ix)
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .px(px(10.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .text_size(px(11.5))
                    .cursor_pointer()
                    .text_color(if active { p.text } else { p.text_dim })
                    .when(active, |s| s.bg(p.surface).shadow_xs())
                    .when(!active, |s| s.hover(|s| s.text_color(p.text)))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_script_stage(stage, cx)))
                    .child(stage.label())
                    .when(filled, |s| {
                        s.child(
                            div()
                                .id(("script-stage-dot", ix))
                                .size(px(5.))
                                .rounded_full()
                                .bg(p.accent)
                                .test_support(),
                        )
                    })
                    .test_support()
            }))
            .test_support();

        let editor = div()
            .id("script-editor")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .border_t_1()
            .border_color(p.border)
            .child(
                Editor::new(self.script_editor(current))
                    .appearance(false)
                    .bordered(false)
                    .font_family(MONO)
                    .text_size(px(12.))
                    .h(relative(1.)),
            )
            .test_support();

        let hint = div()
            .id("script-hint")
            .flex_none()
            .px(px(14.))
            .py(px(5.))
            .border_t_1()
            .border_color(p.border)
            .text_size(px(11.))
            .text_color(p.text_dim)
            .child("Postman-совместимый API: pm.environment, pm.response, pm.test, console.log")
            .test_support();

        let snippets = div()
            .id("script-snippets")
            .flex()
            .flex_col()
            .flex_none()
            .w(px(200.))
            .min_h_0()
            .overflow_y_scroll()
            .border_l_1()
            .border_color(p.border)
            .p(px(8.))
            .gap(px(2.))
            .child(
                div()
                    .px(px(6.))
                    .pb(px(4.))
                    .text_size(px(10.5))
                    .text_color(p.text_dim)
                    .child("Сниппеты"),
            )
            .children(
                current
                    .snippets()
                    .iter()
                    .enumerate()
                    .map(|(ix, (title, _))| {
                        div()
                            .id(("script-snippet", ix))
                            .px(px(6.))
                            .py(px(4.))
                            .rounded(px(5.))
                            .text_size(px(11.5))
                            .text_color(p.accent_hi)
                            .cursor_pointer()
                            .hover(|s| s.bg(p.surface_hi))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.insert_snippet(ix, window, cx)
                            }))
                            .child(*title)
                            .test_support()
                    }),
            );

        div()
            .id("scripts")
            .flex()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .px(px(14.))
                            .py(px(7.))
                            .child(stage_picker),
                    )
                    .child(editor)
                    .child(hint),
            )
            .child(snippets)
    }

    fn render_auth(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *cx.palette();
        let view = cx.entity().downgrade();
        let current = self.auth_kind;

        let label = |text: &'static str| {
            div()
                .w(px(90.))
                .flex_none()
                .text_size(px(12.))
                .text_color(p.text_dim)
                .child(text)
        };
        let field_box = || {
            div()
                .flex()
                .flex_1()
                .max_w(px(460.))
                .items_center()
                .h(px(32.))
                .px(px(4.))
                .rounded(px(7.))
                .bg(p.surface)
                .border_1()
                .border_color(p.border)
        };
        let input = |state: &Entity<InputState>, id: &'static str, mono: bool| {
            Input::new(state)
                .id(id)
                .appearance(false)
                .text_size(px(12.))
                .h(px(30.))
                .when(mono, |i| i.font_family(MONO))
                .token(|ctx, _, cx| var_token_element(ctx, cx))
        };
        let row = |l: &'static str| div().flex().items_center().gap(px(12.)).child(label(l));
        let hint = |text: String| {
            div()
                .pl(px(102.))
                .text_size(px(11.5))
                .text_color(p.text_dim)
                .child(text)
        };

        let trigger = div()
            .id("auth-type")
            .flex()
            .items_center()
            .justify_between()
            .w(px(260.))
            .h(px(32.))
            .px(px(10.))
            .rounded(px(7.))
            .bg(p.surface)
            .border_1()
            .border_color(p.border)
            .text_size(px(12.))
            .cursor_pointer()
            .hover(|s| s.bg(p.surface_hi))
            .on_click(cx.listener(|this, _, _, cx| {
                this.auth_menu_open = !this.auth_menu_open;
                cx.notify();
            }))
            .child(current.label())
            .child(icon(IconName::ChevronDown, 12., p.text_dim))
            .test_support();
        let menu =
            div()
                .flex()
                .flex_col()
                .p(px(4.))
                .children(AuthKind::ALL.into_iter().enumerate().map(|(ix, k)| {
                    let view = view.clone();
                    div()
                        .id(("auth-option", ix))
                        .flex()
                        .items_center()
                        .justify_between()
                        .h(px(28.))
                        .px(px(8.))
                        .rounded(px(6.))
                        .text_size(px(12.))
                        .cursor_pointer()
                        .hover(|s| s.bg(p.surface_hi))
                        .on_click(move |_, _, cx| {
                            view.update(cx, |this, cx| this.set_auth_kind(k, cx)).ok();
                        })
                        .child(k.label())
                        .when(k == current, |d| {
                            d.child(icon(IconName::Check, 13., p.accent))
                        })
                        .test_support()
                }));
        let dismiss = view.clone();
        let picker = Popover::new("auth-type-menu")
            .open(self.auth_menu_open)
            .width(260.)
            .trigger(trigger)
            .child(menu)
            .on_dismiss(move |_, cx| {
                dismiss
                    .update(cx, |this, cx| {
                        this.auth_menu_open = false;
                        cx.notify();
                    })
                    .ok();
            });

        let mut body = div()
            .id("auth")
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(14.))
            .child(row("Type").child(picker));

        match current {
            AuthKind::None => {
                body = body.child(hint("Запрос будет отправлен без авторизации.".into()));
            }
            AuthKind::Inherit => {
                body = body.child(hint(
                    "Схема безопасности берётся из спецификации; токен — из переменной {{token}} активного окружения."
                        .into(),
                ));
            }
            AuthKind::Bearer => {
                let token = self.token.read(cx).value().to_string();
                let field = if self.token_revealed {
                    input(&self.token, "auth-token", true).into_any_element()
                } else {
                    div()
                        .id("auth-token-masked")
                        .flex_1()
                        .px(px(8.))
                        .font_family(MONO)
                        .text_size(px(12.))
                        .cursor_text()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.token_revealed = true;
                            this.token.update(cx, |s, cx| s.focus(window, cx));
                            cx.notify();
                        }))
                        .child(mask_secret(&token))
                        .test_support()
                        .into_any_element()
                };
                body = body
                    .child(
                        row("Token").child(
                            field_box()
                                .child(div().flex_1().min_w_0().child(field))
                                .child(
                                    IconButton::new(
                                        "auth-token-eye",
                                        if self.token_revealed {
                                            IconName::EyeOff
                                        } else {
                                            IconName::Eye
                                        },
                                    )
                                    .size(13.)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.toggle_token_revealed(cx)
                                        }),
                                    ),
                                ),
                        ),
                    )
                    .child(hint(
                        "Заголовок Authorization: Bearer •••••••• добавится автоматически".into(),
                    ));
            }
            AuthKind::Basic => {
                body = body
                    .child(row("Username").child(field_box().child(input(
                        &self.username,
                        "auth-username",
                        true,
                    ))))
                    .child(
                        row("Password").child(
                            field_box().child(
                                Input::new(&self.password)
                                    .id("auth-password")
                                    .appearance(false)
                                    .mask_toggle()
                                    .font_family(MONO)
                                    .text_size(px(12.))
                                    .h(px(30.)),
                            ),
                        ),
                    )
                    .child(hint(
                        "Заголовок Authorization: Basic •••••••• добавится автоматически".into(),
                    ));
            }
            AuthKind::ApiKey => {
                let key = self.api_key.read(cx).value().to_string();
                let key = if key.is_empty() {
                    "<key>".to_string()
                } else {
                    key
                };
                let where_ = if self.api_in_query {
                    format!("Параметр ?{key}=•••• добавится к URL автоматически")
                } else {
                    format!("Заголовок {key}: •••• добавится автоматически")
                };
                let view = view.clone();
                body = body
                    .child(row("Key").child(field_box().child(input(
                        &self.api_key,
                        "auth-api-key",
                        true,
                    ))))
                    .child(row("Value").child(field_box().child(input(
                        &self.api_value,
                        "auth-api-value",
                        true,
                    ))))
                    .child(
                        row("Add to").child(
                            Segmented::new("auth-api-in")
                                .items(["Header", "Query"])
                                .selected(self.api_in_query as usize)
                                .on_select(move |ix, _, cx| {
                                    view.update(cx, |this, cx| {
                                        this.set_api_key_in_query(ix == 1, cx)
                                    })
                                    .ok();
                                }),
                        ),
                    )
                    .child(hint(where_));
            }
        }
        body
    }
}

impl Render for RequestEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Repaint variable highlights in the new palette after a theme switch.
        if self
            .body_vars
            .as_ref()
            .is_some_and(|(_, _, mode)| *mode != cx.palette().mode)
        {
            self.highlight_body_vars(cx);
        }
        self.sync_expanded_value(window, cx);
        let p = *cx.palette();
        let content = match self.tab {
            RequestEditorTab::Params => self.render_params(cx),
            RequestEditorTab::Headers => self.render_kv(KvTarget::Headers, cx).into_any_element(),
            RequestEditorTab::Body => self.render_kv(KvTarget::Params, cx).into_any_element(),
            RequestEditorTab::Auth => self.render_auth(cx).into_any_element(),
            RequestEditorTab::Scripts => self.render_scripts(cx).into_any_element(),
            RequestEditorTab::Docs => div()
                .id("request-docs")
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(
                    Editor::new(&self.docs_editor)
                        .appearance(false)
                        .bordered(false)
                        .text_size(px(12.5))
                        .h(relative(1.)),
                )
                .test_support()
                .into_any_element(),
            RequestEditorTab::Settings => {
                let view = cx.entity();
                let ssl_view = view.clone();
                let (default_timeout, default_ssl) = self.connection_defaults;
                div()
                    .id("request-settings")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(RequestRedirectSettings::new(
                        self.spec.follow_redirects,
                        self.redirect_defaults.0,
                        self.redirect_defaults.1,
                        self.redirect_limit.clone(),
                        move |follow, _, cx| {
                            view.update(cx, |this, cx| this.set_follow_redirects(follow, cx));
                        },
                    ))
                    .child(
                        div()
                            .id("request-connection-settings")
                            .flex()
                            .flex_col()
                            .gap(px(16.))
                            .px(px(18.))
                            .pb(px(18.))
                            .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child("Соединение"))
                            .child(div().text_size(px(12.)).text_color(p.text_dim).child(format!(
                                "Настройка приложения: таймаут {default_timeout} мс · сертификаты SSL {}",
                                if default_ssl { "проверяются" } else { "не проверяются" }
                            )))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(12.))
                                    .child(div().text_size(px(12.)).child("Проверка SSL"))
                                    .child(
                                        Segmented::new("request-ssl-verify")
                                            .items(["По умолчанию", "Проверять", "Не проверять"])
                                            .selected(match self.spec.ssl_verify {
                                                None => 0,
                                                Some(true) => 1,
                                                Some(false) => 2,
                                            })
                                            .on_select(move |index, _, cx| {
                                                let verify = match index {
                                                    1 => Some(true),
                                                    2 => Some(false),
                                                    _ => None,
                                                };
                                                ssl_view.update(cx, |this, cx| this.set_ssl_verify(verify, cx));
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(12.))
                                    .child(div().text_size(px(12.)).child("Таймаут, мс"))
                                    .child(div().w(px(160.)).child(
                                        Input::new(&self.request_timeout).id("request-timeout").h(px(30.)),
                                    )),
                            )
                            .child(div().text_size(px(11.)).text_color(p.text_dim).child(
                                "Пусто — значение приложения. От 1 до 3 600 000 мс.",
                            ))
                            .test_support(),
                    )
                    .into_any_element()
            }
        };
        let available = self
            .split_bounds
            .get()
            .map(|b| b.size.height.as_f32())
            .unwrap_or_else(|| window.viewport_size().height.as_f32());
        let top_height = details_height(available, self.split_ratio);
        let bounds = self.split_bounds.clone();
        div()
            .id("request-editor")
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .bg(p.bg)
            .text_color(p.text)
            .on_mouse_move(cx.listener(|this, event, _, cx| this.move_split(event, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.stop_split(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.stop_split(cx)),
            )
            .child(
                canvas(
                    move |measured, _, _| bounds.set(Some(measured)),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(
                div()
                    .id("request-details-pane")
                    .flex()
                    .flex_col()
                    .flex_none()
                    .h(px(top_height))
                    .min_h(px(MIN_DETAILS_HEIGHT))
                    .min_w_0()
                    .child(self.render_header(cx))
                    .child(content)
                    .test_support(),
            )
            .child(
                HorizontalPaneSplitter::new("request-body-divider")
                    .active(self.split_dragging)
                    .on_drag_start(cx.listener(|this, _, _, cx| {
                        this.split_dragging = true;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("request-body-pane")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(MIN_BODY_HEIGHT))
                    .min_w_0()
                    .child(
                        div()
                            .id(("subtab", RequestEditorTab::Body as usize))
                            .flex()
                            .flex_none()
                            .items_center()
                            .h(px(36.))
                            .px(px(14.))
                            .border_b_1()
                            .border_color(p.border)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Body")
                            .test_support(),
                    )
                    .child(self.render_body(cx))
                    .test_support(),
            )
    }
}
