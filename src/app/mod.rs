//! Main window. `Workspace` owns the project and all UI state; regions are
//! rendered by `impl Workspace` blocks in sibling modules and by child panel
//! entities (request editor, response view, env/history panels, palette).

pub mod browser;
#[cfg(windows)]
mod browser_win;
mod collection_dialog;
pub mod collection_panel;
pub mod env_panel;
pub mod history_panel;
pub mod menu;
pub mod palette;
mod request;
pub mod request_editor;
pub mod response_view;
pub mod settings_panel;
mod sidebar;
mod statusbar;
mod titlebar;
pub mod update_ui;
pub mod ws_view;

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::menu::AppMenuBar;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use self::collection_dialog::{CollectionDialog, DialogMode};
use self::collection_panel::{CollectionPanel, CollectionPanelEvent};
use self::env_panel::{EnvPanel, EnvPanelEvent};
use self::history_panel::{HistoryPanel, HistoryPanelEvent};
use self::palette::{CommandPalette, PaletteEvent, PaletteItem};
use self::request_editor::{RequestEditor, RequestEditorEvent};
use self::response_view::{ResponseState, ResponseView, ResponseViewEvent};
use self::settings_panel::{CheckForUpdates, SettingsPanel, SettingsPanelEvent};
pub use self::update_ui::{UpdateHooks, UpdateState};
use self::ws_view::{WsView, WsViewEvent};
use crate::core::openapi::Spec;
use crate::core::postman::{self, PostmanImport};
use crate::core::{
    Collection, Environment, HistoryEntry, Method, Project, RequestSpec, SendError, SpecGroup,
    curl, http, new_id, runner, store, vars, ws,
};
use crate::model::RailMode;
use crate::theme::{ActivePalette, Palette};
use crate::ui::{shortcut_hint, tokenize_vars, var_content};

actions!(
    rivet,
    [
        NewTab,
        CloseTab,
        ForceCloseTab,
        NextTab,
        PrevTab,
        SendRequest,
        CancelRequest,
        SaveRequest,
        CopyCurl,
        OpenPalette,
        OpenSpec,
        ReloadSpec,
        SaveProject,
        ToggleTheme,
        ToggleEnv,
        ToggleSidebar,
        ShowRequests,
        ShowCollections,
        ShowEnvs,
        ShowHistory,
        ShowSettings,
        Quit,
    ]
);

/// Height of the custom titlebar; traffic lights are centred against it.
pub const TITLEBAR_HEIGHT: f32 = 44.;
const MODE_RAIL_WIDTH: f32 = 48.;
/// Native close/minimize/zoom button height incl. AppKit's frame padding.
const TRAFFIC_LIGHT_HEIGHT: f32 = 16.;
const SAVE_DEBOUNCE: Duration = Duration::from_millis(400);
/// How often project.json is checked for changes made by other processes
/// (e.g. `rivet mcp`).
pub const EXTERNAL_POLL: Duration = Duration::from_secs(1);
/// Status bar notice after merging an external change of project.json.
pub const EXTERNAL_NOTICE: &str = "Проект обновлён извне";

pub fn window_options(cx: &App) -> WindowOptions {
    let base = gpui_kit::component::TitleBar::window_options();
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            traffic_light_position: Some(point(
                px(16.),
                px((TITLEBAR_HEIGHT - TRAFFIC_LIGHT_HEIGHT) / 2.),
            )),
            ..base.titlebar.unwrap_or_default()
        }),
        window_bounds: Some(WindowBounds::centered(size(px(1440.), px(900.)), cx)),
        window_min_size: Some(size(px(960.), px(600.))),
        ..base
    }
}

pub fn bind_keys(cx: &mut App) {
    let ws = Some("Workspace");
    cx.bind_keys([
        KeyBinding::new("secondary-t", NewTab, ws),
        KeyBinding::new("secondary-w", CloseTab, ws),
        KeyBinding::new("secondary-alt-w", ForceCloseTab, ws),
        KeyBinding::new("ctrl-tab", NextTab, ws),
        KeyBinding::new("ctrl-shift-tab", PrevTab, ws),
        KeyBinding::new("secondary-enter", SendRequest, ws),
        KeyBinding::new("secondary-.", CancelRequest, ws),
        KeyBinding::new("secondary-s", SaveRequest, ws),
        KeyBinding::new("secondary-shift-c", CopyCurl, ws),
        KeyBinding::new("secondary-k", OpenPalette, ws),
        KeyBinding::new("secondary-p", OpenPalette, ws),
        KeyBinding::new("secondary-o", OpenSpec, ws),
        KeyBinding::new("secondary-r", ReloadSpec, ws),
        KeyBinding::new("secondary-shift-l", ToggleTheme, ws),
        KeyBinding::new("secondary-e", ToggleEnv, ws),
        KeyBinding::new("secondary-b", ToggleSidebar, ws),
        KeyBinding::new("secondary-1", ShowRequests, ws),
        KeyBinding::new("secondary-2", ShowEnvs, ws),
        KeyBinding::new("secondary-3", ShowHistory, ws),
        KeyBinding::new("secondary-4", ShowCollections, ws),
    ]);
    palette::bind_keys(cx);
    gpui_kit::base::GlobalState::global_mut(cx)
        .set_app_menus(menu::app_menus().into_iter().map(Menu::owned).collect());
}

/// Methods that change server state and need confirmation in production.
pub fn is_destructive(m: Method) -> bool {
    matches!(
        m,
        Method::Post | Method::Put | Method::Patch | Method::Delete
    )
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Stamp first, then content: a write in between changes the stamp again, so
/// the next poll still notices it.
fn disk_fingerprint(path: &std::path::Path) -> (Option<store::FileStamp>, Option<u64>) {
    let stamp = store::file_stamp(path);
    let hash = std::fs::read(path).ok().map(|bytes| store::content_hash(&bytes));
    (stamp, hash)
}

fn find_request<'a>(project: &'a Project, id: &str) -> Option<(&'a str, &'a RequestSpec)> {
    project.collections.iter().find_map(|c| {
        c.requests
            .iter()
            .find(|r| r.id == id)
            .map(|r| (c.id.as_str(), r))
    })
}

/// Same request content, ignoring the owning-collection back-reference (tabs
/// opened from a collection carry it, stored copies may not).
fn same_request(a: &RequestSpec, b: &RequestSpec) -> bool {
    let mut a = a.clone();
    let mut b = b.clone();
    a.collection_id = None;
    b.collection_id = None;
    a == b
}

fn host_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or(rest)
        .to_string()
}

/// Toast after a send with undefined `{{vars}}` (they went out empty).
fn missing_vars_notice(names: &[String]) -> String {
    let list: Vec<String> = names.iter().map(|n| format!("{{{{{n}}}}}")).collect();
    format!("Не определены {} — отправлены пустой строкой", list.join(", "))
}

fn request_name_from_url(url: &str) -> String {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .trim_end_matches('/');
    let segment = path.rsplit('/').next().unwrap_or("").trim();
    if segment.is_empty() || segment.starts_with("{{") {
        "Untitled".into()
    } else {
        segment.to_string()
    }
}

/// Choices in the "unsaved changes" prompt when closing a tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseChoice {
    Save,
    Discard,
    Cancel,
}

pub struct RequestTab {
    pub request: RequestSpec,
    pub response: ResponseState,
    /// Resolved URL of the last send (base for Browser mode).
    pub last_url: Option<String>,
    /// Tests, console output and errors of the last send's scripts.
    pub script_report: Option<crate::core::ScriptReport>,
    /// Cancels the in-flight send; dropping the task detaches the result.
    inflight: Option<(http::CancelToken, Task<()>)>,
    /// What the tab started from, for "unsaved changes" of requests that are
    /// not in any collection (new, from history, from a spec).
    baseline: RequestSpec,
}

impl RequestTab {
    pub fn new(request: RequestSpec) -> Self {
        Self {
            baseline: request.clone(),
            request,
            response: ResponseState::Empty,
            last_url: None,
            script_report: None,
            inflight: None,
        }
    }

    pub fn is_sending(&self) -> bool {
        self.inflight.is_some()
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum SidebarMenu {
    Collection(String),
    Folder(String, String),
    Request(String),
}

pub struct Workspace {
    pub focus: FocusHandle,
    pub rail: RailMode,
    pub project: Project,
    /// Where the project is saved; `None` keeps everything in memory (tests).
    pub store_path: Option<PathBuf>,
    pub spec: Option<Arc<Spec>>,
    linked_specs: HashMap<String, Arc<Spec>>,
    pub spec_error: Option<String>,
    /// Collapsed sidebar folders by name ("spec:<group>" / "col:<id>").
    pub collapsed: HashSet<String>,
    pub sidebar_visible: bool,
    app_menu_bar: Option<Entity<AppMenuBar>>,
    /// Request/response pane split, measured against the available content width.
    request_split_ratio: f32,
    /// Sidebar width (collections / history), dragged by its splitter.
    pub sidebar_width: f32,
    /// Drag in progress: (cursor x at start, width at start).
    sidebar_drag: Option<(f32, f32)>,
    sidebar_view: Entity<sidebar::SidebarView>,
    /// [`Self::sidebar_key`] the cached sidebar was last rendered for.
    sidebar_key: Option<u64>,
    request_split_dragging: bool,
    request_split_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub tabs: Vec<RequestTab>,
    pub active_tab: Option<usize>,
    /// One URL editor shared by all tabs; reloaded on switch.
    pub url_input: Entity<InputState>,
    pub filter_input: Entity<InputState>,
    /// Lower-cased sidebar filter, cached from `filter_input`.
    pub filter: String,
    pub method_menu_open: bool,
    /// "+" in the tab bar: HTTP request / WebSocket.
    pub new_tab_menu_open: bool,
    pub tab_menu: Option<usize>,
    collection_menu: Option<SidebarMenu>,
    /// Collection id whose sidebar "Удалить коллекцию" confirmation is open.
    collection_delete_confirm: Option<String>,
    collection_dialog: Option<Entity<CollectionDialog>>,
    pub env_menu_open: bool,
    /// Production confirmation popover next to Send.
    pub confirm_open: bool,
    pub editor: Entity<RequestEditor>,
    pub response: Entity<ResponseView>,
    /// Body of WebSocket tabs (owns their live connections).
    pub ws_view: Entity<WsView>,
    pub env_panel: Entity<EnvPanel>,
    pub collection_panel: Entity<CollectionPanel>,
    pub settings_panel: Entity<SettingsPanel>,
    http_sessions: http::HttpSessions,
    pub history_panel: Entity<HistoryPanel>,
    pub palette: Option<Entity<CommandPalette>>,
    /// Tab whose close is waiting for "save / don't save / cancel".
    pub close_confirm: Option<usize>,
    /// Scripts changed environment variables; the env panel must reload them.
    env_panel_stale: bool,
    /// Transient message in the status bar ("Сохранено", "cURL скопирован").
    pub notice: Option<SharedString>,
    notice_generation: u64,
    save_task: Option<Task<()>>,
    notice_task: Option<Task<()>>,
    /// Project as last read from / written to `store_path`: the base of the
    /// three-way merge with external changes.
    synced: Project,
    /// Stamp and content hash of the store file as the app last saw it, so its
    /// own saves are not mistaken for external changes.
    disk_stamp: Option<store::FileStamp>,
    disk_hash: Option<u64>,
    /// An external change was merged: panels and the active tab's views must be
    /// reloaded on the next render (they need a window).
    external_stale: bool,
    /// The active tab's request was refreshed from disk.
    active_stale: bool,
    _poll_task: Option<Task<()>>,
    /// Self-update (see `update_ui`): state, side effects, popover.
    pub update_state: UpdateState,
    pub update_hooks: UpdateHooks,
    pub update_popover_open: bool,
    update_cancel: Option<http::CancelToken>,
    update_task: Option<Task<()>>,
    _update_progress: Option<Task<()>>,
    _update_timer: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(
        project: Project,
        store_path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let project_sidebar_width = project
            .sidebar_width
            .map(|w| w as f32)
            .unwrap_or(sidebar::SIDEBAR_DEFAULT_WIDTH)
            .clamp(sidebar::SIDEBAR_MIN_WIDTH, sidebar::SIDEBAR_MAX_WIDTH);
        window.focus(&focus, cx);

        let (spec, spec_error) = match &project.spec_path {
            Some(path) => match Spec::load(path) {
                Ok(spec) => (Some(Arc::new(spec)), None),
                Err(e) => (None, Some(e)),
            },
            None => (None, None),
        };
        let linked_specs = project
            .collections
            .iter()
            .filter_map(|collection| {
                let source = collection.source.as_ref()?;
                Spec::load(source)
                    .ok()
                    .map(|spec| (collection.id.clone(), Arc::new(spec)))
            })
            .collect();

        let mut tabs: Vec<RequestTab> = project
            .open_tabs
            .iter()
            .cloned()
            .map(RequestTab::new)
            .collect();
        if tabs.is_empty() {
            if let Some(ep) = spec
                .as_ref()
                .and_then(|s| s.groups.first()?.endpoints.first().cloned())
            {
                tabs.push(RequestTab::new(ep.to_request()));
            } else {
                tabs.push(RequestTab::new(RequestSpec::untitled()));
            }
        }
        let active_tab = Some(project.active_tab.unwrap_or(0).min(tabs.len() - 1));
        let first = tabs[active_tab.unwrap()].request.clone();

        let url_input = cx.new(|cx| {
            let mut state =
                InputState::new(window, cx).placeholder("Введите URL или вставьте cURL");
            state.set_value(var_content(&first.url), window, cx);
            state
        });
        let filter_input = cx.new(|cx| InputState::new(window, cx).placeholder("Поиск запросов"));
        let ws_view = {
            let first = first.clone();
            cx.new(|cx| WsView::new(first, window, cx))
        };
        let editor = cx.new(|cx| RequestEditor::new(first, window, cx));
        editor.update(cx, |editor, cx| {
            editor.set_redirect_defaults(
                project.settings.follow_redirects,
                project.settings.max_redirects,
                cx,
            )
        });
        let response = cx.new(|cx| ResponseView::new(window, cx));
        let env_panel = {
            let envs = project.environments.clone();
            let active = project.active_env;
            cx.new(|cx| EnvPanel::new(envs, active, window, cx))
        };
        let history_panel = cx.new(|cx| HistoryPanel::new(window, cx));
        let collection_panel =
            cx.new(|cx| CollectionPanel::new(project.collections.clone(), window, cx));
        let settings_panel = cx.new(|cx| {
            let mut panel = SettingsPanel::new(
                project.settings.clone(),
                store_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "В памяти".into()),
                window,
                cx,
            );
            panel.set_project_path(store_path.clone(), cx);
            panel
        });
        history_panel.update(cx, |p, cx| p.set_entries(project.history.clone(), cx));
        let app_menu_bar = (!cfg!(target_os = "macos")).then(|| AppMenuBar::new(cx));
        let (disk_stamp, disk_hash) = match &store_path {
            Some(path) => disk_fingerprint(path),
            None => (None, None),
        };
        let poll_task = store_path.is_some().then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(EXTERNAL_POLL).await;
                    if this.update(cx, |this, cx| this.poll_external(cx)).is_err() {
                        break;
                    }
                }
            })
        });

        let subscriptions = vec![
            cx.subscribe_in(&url_input, window, Self::on_url_event),
            cx.subscribe_in(
                &filter_input,
                window,
                |this, state, ev: &InputEvent, _, cx| {
                    if let InputEvent::Change = ev {
                        this.filter = state.read(cx).value().trim().to_lowercase();
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(&editor, window, Self::on_editor_event),
            cx.subscribe_in(&response, window, Self::on_response_event),
            cx.subscribe_in(&ws_view, window, Self::on_ws_event),
            // The request line shows the connection state.
            cx.observe(&ws_view, |_, _, cx| cx.notify()),
            cx.subscribe_in(&env_panel, window, Self::on_env_panel_event),
            cx.subscribe_in(&history_panel, window, Self::on_history_event),
            cx.subscribe_in(&collection_panel, window, Self::on_collection_panel_event),
            cx.subscribe_in(&settings_panel, window, Self::on_settings_panel_event),
            cx.subscribe(&settings_panel, |this, _, _: &CheckForUpdates, cx| {
                this.check_for_updates(true, cx)
            }),
            // The debounced save may still be pending when the app quits.
            cx.on_app_quit(|this, _| {
                this.save_now();
                async {}
            }),
        ];

        let mut this = Self {
            focus,
            rail: RailMode::Requests,
            synced: project.clone(),
            project,
            store_path,
            spec,
            linked_specs,
            spec_error,
            collapsed: HashSet::new(),
            sidebar_visible: true,
            app_menu_bar,
            request_split_ratio: 0.5,
            sidebar_width: project_sidebar_width,
            sidebar_drag: None,
            sidebar_view: {
                let workspace = cx.entity().downgrade();
                cx.new(|_| sidebar::SidebarView { workspace })
            },
            sidebar_key: None,
            request_split_dragging: false,
            request_split_bounds: Rc::new(Cell::new(None)),
            tabs,
            active_tab,
            url_input,
            filter_input,
            filter: String::new(),
            method_menu_open: false,
            new_tab_menu_open: false,
            tab_menu: None,
            collection_menu: None,
            collection_delete_confirm: None,
            collection_dialog: None,
            env_menu_open: false,
            confirm_open: false,
            editor,
            response,
            ws_view,
            env_panel,
            collection_panel,
            settings_panel,
            http_sessions: http::HttpSessions::default(),
            history_panel,
            palette: None,
            close_confirm: None,
            env_panel_stale: false,
            notice: None,
            notice_generation: 0,
            save_task: None,
            notice_task: None,
            disk_stamp,
            disk_hash,
            external_stale: false,
            active_stale: false,
            _poll_task: poll_task,
            update_state: UpdateState::Idle,
            update_hooks: UpdateHooks::production(),
            update_popover_open: false,
            update_cancel: None,
            update_task: None,
            _update_progress: None,
            _update_timer: None,
            _subscriptions: subscriptions,
        };
        this.restart_update_timer(cx);
        this.sync_update_status(cx);
        this
    }

    // ---- accessors -------------------------------------------------------

    pub fn active(&self) -> Option<&RequestSpec> {
        self.active_tab
            .and_then(|ix| self.tabs.get(ix))
            .map(|t| &t.request)
    }

    fn active_mut(&mut self) -> Option<&mut RequestTab> {
        self.active_tab.and_then(|ix| self.tabs.get_mut(ix))
    }

    pub fn env(&self) -> Option<&Environment> {
        self.project.environments.get(self.project.active_env)
    }

    fn spec_for_request(&self, request: &RequestSpec) -> Option<&Spec> {
        if let Some(id) = request.collection_id.as_ref() {
            if self
                .project
                .collections
                .iter()
                .any(|c| &c.id == id && c.source.is_some())
            {
                return self.linked_specs.get(id).map(Arc::as_ref);
            }
        }
        self.spec.as_deref()
    }

    /// Spec operation `request` is linked to (field popovers, body schema checks).
    fn schema_link_for(&self, request: &RequestSpec) -> Option<crate::core::schema_hover::SchemaLink> {
        let spec_ref = request.spec_ref.clone()?;
        let linked = request.collection_id.as_ref().filter(|id| {
            self.project.collections.iter().any(|c| &c.id == *id && c.source.is_some())
        });
        let spec = match linked {
            Some(id) => self.linked_specs.get(id)?.clone(),
            None => self.spec.clone()?,
        };
        Some(crate::core::schema_hover::SchemaLink::new(spec, spec_ref))
    }

    /// Collection variables override environment variables for that request.
    fn request_env(&self, request: &RequestSpec) -> Option<Environment> {
        let collection = request
            .collection_id
            .as_ref()
            .and_then(|id| self.project.collections.iter().find(|c| &c.id == id));
        let mut env = self.env().cloned().or_else(|| {
            collection.map(|c| Environment::new(&c.name, crate::core::EnvColor::Blue))
        })?;
        if let Some(collection) = collection {
            for var in &collection.vars {
                env.vars.retain(|old| old.key != var.key);
                env.vars.push(var.clone());
            }
        }
        Some(env)
    }

    /// Second half of an external merge, run on render (inputs need a window).
    fn reload_external_views(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let collections = self.project.collections.clone();
        self.collection_panel
            .update(cx, |p, cx| p.set_collections(collections, window, cx));
        let settings = self.project.settings.clone();
        self.editor.update(cx, |e, cx| {
            e.set_redirect_defaults(settings.follow_redirects, settings.max_redirects, cx)
        });
        self.settings_panel
            .update(cx, |p, cx| p.set_settings(settings, window, cx));
        let history = self.project.history.clone();
        self.history_panel.update(cx, |p, cx| p.set_entries(history, cx));
        if std::mem::take(&mut self.active_stale) {
            self.load_active_views(window, cx);
        }
    }

    pub fn spec_groups(&self) -> &[SpecGroup] {
        self.spec
            .as_ref()
            .map(|s| s.groups.as_slice())
            .unwrap_or(&[])
    }

    // ---- persistence -----------------------------------------------------

    /// Debounced save of the whole project.
    pub fn schedule_save(&mut self, cx: &mut Context<Self>) {
        if self.store_path.is_none() {
            return;
        }
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            this.update(cx, |this, cx| match this.persist_project() {
                Ok(true) => this.notify_user(EXTERNAL_NOTICE, cx),
                Ok(false) => {}
                Err(e) => eprintln!("rivet: failed to save project: {e}"),
            })
            .ok();
        }));
    }

    pub fn save_now(&mut self) {
        if let Err(e) = self.persist_project() {
            eprintln!("rivet: failed to save project: {e}");
        }
    }

    /// Writes the project. External changes made since the last load/save are
    /// merged first, so a save never discards them. Returns whether a merge
    /// happened.
    fn persist_project(&mut self) -> Result<bool, String> {
        let merged = self.absorb_external().is_some();
        self.project.open_tabs = self.tabs.iter().map(|t| t.request.clone()).collect();
        self.project.active_tab = self.active_tab;
        if let Some(path) = self.store_path.clone() {
            let text = store::save_text(&path, &self.project)?;
            self.note_written(&path, &text);
            self.synced = self.project.clone();
        }
        Ok(merged)
    }

    /// Remembers what the app just wrote. If the file already differs from it
    /// (another process wrote in between), the stamp is left unknown so the
    /// next poll merges that write.
    fn note_written(&mut self, path: &std::path::Path, text: &str) {
        let hash = store::content_hash(text.as_bytes());
        let (stamp, on_disk) = disk_fingerprint(path);
        self.disk_hash = Some(hash);
        self.disk_stamp = if on_disk == Some(hash) { stamp } else { None };
    }

    /// Timer tick: merges project.json if another process changed it.
    fn poll_external(&mut self, cx: &mut Context<Self>) {
        let Some(needs_write) = self.absorb_external() else {
            return;
        };
        if needs_write {
            self.schedule_save(cx);
        }
        self.notify_user(EXTERNAL_NOTICE, cx);
    }

    /// Reads project.json if it changed since the app last loaded or wrote it,
    /// and merges it into the running workspace. Returns `None` when nothing
    /// changed, else whether the merged project still differs from the file
    /// (app-side changes that need a save).
    fn absorb_external(&mut self) -> Option<bool> {
        let path = self.store_path.clone()?;
        let stamp = store::file_stamp(&path)?;
        if self.disk_stamp == Some(stamp) {
            return None;
        }
        let bytes = std::fs::read(&path).ok()?;
        let hash = store::content_hash(&bytes);
        self.disk_stamp = Some(stamp);
        if self.disk_hash == Some(hash) {
            return None;
        }
        // Unparsable (e.g. written non-atomically, mid-write): wait for the
        // next change; the stamp is recorded so it isn't re-read every tick.
        let disk: Project = serde_json::from_slice(&bytes).ok()?;
        self.disk_hash = Some(hash);
        let merged = store::merge_projects(&self.synced, &self.project, &disk);
        let old = std::mem::replace(&mut self.project, merged);
        self.synced = disk;
        self.reconcile_external(&old);
        let mut on_disk = self.synced.clone();
        on_disk.open_tabs = self.project.open_tabs.clone();
        on_disk.active_tab = self.project.active_tab;
        Some(on_disk != self.project)
    }

    /// Brings UI state in line with a merged project (`old` is the project
    /// before the merge). Open tabs are kept; a tab without unsaved edits (equal
    /// to its saved copy) is refreshed from its collection.
    fn reconcile_external(&mut self, old: &Project) {
        let envs = self.project.environments.len();
        self.project.active_env = self.project.active_env.min(envs.saturating_sub(1));
        let project = &self.project;
        for (ix, tab) in self.tabs.iter_mut().enumerate() {
            match find_request(project, &tab.request.id) {
                Some((collection_id, fresh)) => {
                    let clean = find_request(old, &tab.request.id)
                        .is_some_and(|(_, saved)| same_request(saved, &tab.request));
                    if clean && !same_request(fresh, &tab.request) {
                        let mut fresh = fresh.clone();
                        fresh.collection_id = Some(collection_id.to_string());
                        tab.request = fresh;
                        self.active_stale |= self.active_tab == Some(ix);
                    }
                }
                None => {
                    // Its collection was deleted: detach, as delete_collection does.
                    let gone = tab.request.collection_id.as_ref().is_some_and(|id| {
                        !project.collections.iter().any(|c| &c.id == id)
                    });
                    if gone {
                        tab.request.collection_id = None;
                        tab.request.folder = None;
                        self.active_stale |= self.active_tab == Some(ix);
                    }
                }
            }
        }

        let exists = |id: &str| project.collections.iter().any(|c| c.id == id);
        self.collapsed.retain(|key| match key.strip_prefix("col:") {
            Some(rest) => exists(rest.split(':').next().unwrap_or(rest)),
            None => true,
        });
        if self
            .collection_delete_confirm
            .as_deref()
            .is_some_and(|id| !exists(id))
        {
            self.collection_delete_confirm = None;
        }
        if matches!(
            &self.collection_menu,
            Some(SidebarMenu::Collection(id)) | Some(SidebarMenu::Folder(id, _)) if !exists(id)
        ) {
            self.collection_menu = None;
        }
        for c in &old.collections {
            if !project.collections.iter().any(|n| n.id == c.id && n.cookies_enabled) {
                self.http_sessions.clear(&c.id);
            }
        }
        if old.settings.cookies_enabled && !project.settings.cookies_enabled {
            self.http_sessions.clear_all();
        }

        // Linked specs: drop removed/unlinked collections, load new sources.
        self.linked_specs.retain(|id, _| {
            project
                .collections
                .iter()
                .any(|c| &c.id == id && c.source.is_some())
        });
        for c in &project.collections {
            let Some(source) = &c.source else { continue };
            let unchanged = old
                .collections
                .iter()
                .any(|o| o.id == c.id && o.source.as_ref() == Some(source));
            if !unchanged || !self.linked_specs.contains_key(&c.id) {
                match Spec::load(source) {
                    Ok(spec) => {
                        self.linked_specs.insert(c.id.clone(), Arc::new(spec));
                    }
                    Err(_) => {
                        self.linked_specs.remove(&c.id);
                    }
                }
            }
        }
        if project.spec_path != old.spec_path {
            (self.spec, self.spec_error) = match &project.spec_path {
                Some(path) => match Spec::load(path) {
                    Ok(spec) => (Some(Arc::new(spec)), None),
                    Err(e) => (None, Some(e)),
                },
                None => (None, None),
            };
        }
        self.env_panel_stale = true;
        self.external_stale = true;
    }

    pub fn save_project(&mut self, cx: &mut Context<Self>) {
        if self.store_path.is_none() {
            self.notify_user("Файл проекта не выбран", cx);
            return;
        }
        match self.persist_project() {
            Ok(_) => self.notify_user("Проект сохранён", cx),
            Err(e) => self.notify_user(format!("Не удалось сохранить проект: {e}"), cx),
        }
    }

    pub fn notify_user(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.notice = Some(text.into());
        self.notice_generation = self.notice_generation.wrapping_add(1);
        let generation = self.notice_generation;
        self.notice_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            this.update(cx, |this, cx| {
                if this.notice_generation == generation {
                    this.notice = None;
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    // ---- tabs --------------------------------------------------------------

    /// Pushes the active tab into the shared URL input, editor and response view.
    fn sync_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.method_menu_open = false;
        self.tab_menu = None;
        self.confirm_open = false;
        self.load_active_views(window, cx);
        self.schedule_save(cx);
        cx.notify();
    }

    /// Loads the active tab into the URL input, editor, WebSocket and response views.
    fn load_active_views(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (request, response, last_url, report) = match self.active_tab.and_then(|ix| self.tabs.get(ix)) {
            Some(t) => (t.request.clone(), t.response.clone(), t.last_url.clone(), t.script_report.clone()),
            None => (RequestSpec::untitled(), ResponseState::Empty, None, None),
        };
        self.url_input.update(cx, |s, cx| {
            s.set_value(var_content(&request.url), window, cx)
        });
        if request.is_ws() {
            let request = request.clone();
            self.ws_view
                .update(cx, |v, cx| v.set_spec(request, window, cx));
        }
        let link = self.schema_link_for(&request);
        self.editor.update(cx, |e, cx| {
            e.set_schema_link(link.clone(), cx);
            e.set_spec(request, window, cx)
        });
        self.response.update(cx, |r, cx| {
            r.set_schema_link(link, cx);
            r.set_base_url(last_url, cx);
            r.set_state(response, cx);
            r.set_script_report(report, cx);
        });
    }

    /// Drag & drop in the tab bar: moves tab `from` to position `to`. The
    /// active tab stays active; the order is saved with `open_tabs`.
    pub fn move_tab(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if from == to || from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        let ids: Vec<String> = self.tabs.iter().map(|t| t.request.id.clone()).collect();
        let position = |ix: Option<usize>, tabs: &[RequestTab]| {
            ix.and_then(|ix| ids.get(ix))
                .and_then(|id| tabs.iter().position(|t| &t.request.id == id))
        };
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active_tab = position(self.active_tab, &self.tabs);
        self.close_confirm = position(self.close_confirm, &self.tabs);
        self.tab_menu = None;
        self.schedule_save(cx);
        cx.notify();
    }

    fn set_active(&mut self, ix: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.active_tab = ix;
        self.sync_active(window, cx);
    }

    /// Opens `request` in a new tab, or focuses the tab already showing it
    /// (same id, or same spec operation).
    pub fn open_request(
        &mut self,
        request: RequestSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rail_to_requests();
        let existing = self.tabs.iter().position(|t| {
            t.request.id == request.id
                || (request.spec_ref.is_some()
                    && t.request.spec_ref == request.spec_ref
                    && t.request.collection_id == request.collection_id)
        });
        let ix = existing.unwrap_or_else(|| {
            self.tabs.push(RequestTab::new(request));
            self.tabs.len() - 1
        });
        self.set_active(Some(ix), window, cx);
    }

    pub fn open_spec_endpoint(
        &mut self,
        group: usize,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ep) = self
            .spec_groups()
            .get(group)
            .and_then(|g| g.endpoints.get(idx))
        else {
            return;
        };
        let request = ep.to_request();
        self.open_request(request, window, cx);
    }

    pub fn open_saved(
        &mut self,
        col: usize,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(mut r) = self
            .project
            .collections
            .get(col)
            .and_then(|c| c.requests.get(idx))
            .cloned()
        {
            r.collection_id = Some(self.project.collections[col].id.clone());
            self.open_request(r, window, cx);
        }
    }

    pub fn activate_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.tabs.len() && self.active_tab != Some(ix) {
            self.set_active(Some(ix), window, cx);
        }
    }

    pub fn close_tab_at(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.get(ix).is_some_and(RequestTab::is_sending) {
            self.notify_user(
                "Запрос выполняется — используйте принудительное закрытие",
                cx,
            );
            return;
        }
        if self.tab_unsaved(ix) {
            self.close_confirm = Some(ix);
            cx.notify();
            return;
        }
        self.force_close_tab_at(ix, window, cx);
    }

    /// Whether the tab has changes not saved to its collection (or, for a
    /// request outside collections, changes since it was opened).
    pub fn tab_unsaved(&self, ix: usize) -> bool {
        let Some(tab) = self.tabs.get(ix) else { return false };
        match find_request(&self.project, &tab.request.id) {
            Some((_, saved)) => !same_request(saved, &tab.request),
            None => !same_request(&tab.baseline, &tab.request),
        }
    }

    /// Answer to the "unsaved changes" prompt shown when closing a tab.
    pub fn resolve_close_confirm(&mut self, choice: CloseChoice, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.close_confirm.take() else { return };
        match choice {
            CloseChoice::Cancel => {}
            CloseChoice::Discard => self.force_close_tab_at(ix, window, cx),
            CloseChoice::Save => {
                self.activate_tab(ix, window, cx);
                self.save_request(window, cx);
                // Saved into its collection → close; a new request opens the
                // "save to collection" dialog instead and stays open.
                if !self.tab_unsaved(ix) {
                    self.force_close_tab_at(ix, window, cx);
                }
            }
        }
        cx.notify();
    }

    pub fn force_close_tab_at(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        self.tab_menu = None;
        if let Some((cancel, _)) = self.tabs[ix].inflight.take() {
            cancel.cancel();
        }
        let removed = self.tabs.remove(ix);
        if removed.request.is_ws() {
            self.ws_view
                .update(cx, |v, cx| v.drop_session(&removed.request.id, cx));
        }
        let next = match self.active_tab {
            _ if self.tabs.is_empty() => None,
            Some(a) if a > ix || a == self.tabs.len() => Some(a - 1),
            a => a,
        };
        self.set_active(next, window, cx);
    }

    pub fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rail_to_requests();
        self.tabs.push(RequestTab::new(RequestSpec::untitled()));
        self.set_active(Some(self.tabs.len() - 1), window, cx);
        self.url_input.update(cx, |s, cx| s.focus(window, cx));
    }

    /// Opens an empty WebSocket tab.
    pub fn new_ws_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rail_to_requests();
        self.new_tab_menu_open = false;
        self.tabs.push(RequestTab::new(RequestSpec::untitled_ws()));
        self.set_active(Some(self.tabs.len() - 1), window, cx);
        self.url_input.update(cx, |s, cx| s.focus(window, cx));
    }

    fn cycle_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(a) = self.active_tab {
            let len = self.tabs.len() as isize;
            let next = (a as isize + delta).rem_euclid(len) as usize;
            self.set_active(Some(next), window, cx);
        }
    }

    fn rail_to_requests(&mut self) {
        self.rail = RailMode::Requests;
    }

    // ---- editing -----------------------------------------------------------

    pub fn set_method(&mut self, method: Method, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_mut() {
            tab.request.method = method;
        }
        self.method_menu_open = false;
        self.push_request_to_editor(window, cx);
        self.schedule_save(cx);
        cx.notify();
    }

    fn push_request_to_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(r) = self.active().cloned() {
            let link = self.schema_link_for(&r);
            self.editor.update(cx, |e, cx| {
                e.set_schema_link(link, cx);
                e.set_spec(r, window, cx)
            });
        }
    }

    /// Replaces the active request with a pasted `curl ...` command. The URL
    /// field calls this with the raw clipboard text first (a single-line input
    /// drops the newlines of `\`-continued commands) and again on change.
    pub fn import_curl(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !curl::looks_like_curl(text) {
            return false;
        }
        let Some(mut parsed) = curl::parse_curl(text) else {
            return false;
        };
        if let Some(tab) = self.active_mut() {
            parsed.id = tab.request.id.clone();
            tab.request = parsed;
        }
        self.sync_active(window, cx);
        self.notify_user("cURL импортирован", cx);
        true
    }

    fn on_url_event(
        &mut self,
        state: &Entity<InputState>,
        ev: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            InputEvent::Change => {
                let value = state.read(cx).value();
                // Pasting a cURL command turns it into a request.
                if self.import_curl(&value, window, cx) {
                    return;
                }
                state.update(cx, |s, cx| tokenize_vars(s, window, cx));
                let value = state.read(cx).value().to_string();
                if let Some(tab) = self.active_mut() {
                    if tab.request.url != value {
                        let old_default = request_name_from_url(&tab.request.url);
                        if tab.request.name.is_empty()
                            || tab.request.name == "Untitled"
                            || tab.request.name == old_default
                        {
                            tab.request.name = request_name_from_url(&value);
                        }
                        tab.request.url = value;
                        self.schedule_save(cx);
                    }
                }
            }
            InputEvent::PressEnter { .. } => {
                if self.active().is_some_and(RequestSpec::is_ws) {
                    let live = self
                        .active()
                        .is_some_and(|r| self.ws_view.read(cx).is_live(&r.id));
                    if !live {
                        self.ws_connect(cx);
                    }
                } else {
                    self.send(window, cx)
                }
            }
            _ => {}
        }
    }

    fn on_editor_event(
        &mut self,
        editor: &Entity<RequestEditor>,
        ev: &RequestEditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            RequestEditorEvent::Changed => {
                // URL and method are owned by the request line; take the rest.
                let spec = editor.read(cx).spec().clone();
                if let Some(tab) = self.active_mut() {
                    tab.request.params = spec.params;
                    tab.request.headers = spec.headers;
                    tab.request.body = spec.body;
                    tab.request.auth = spec.auth;
                    tab.request.follow_redirects = spec.follow_redirects;
                    tab.request.max_redirects = spec.max_redirects;
                    tab.request.scripts = spec.scripts;
                }
                self.schedule_save(cx);
                cx.notify();
            }
            RequestEditorEvent::GenerateBody => {
                let body = self.active().and_then(|r| {
                    let spec_ref = r.spec_ref.as_ref()?;
                    self.spec_for_request(r)?.generate_request_body(spec_ref)
                });
                match body {
                    Some(body) => editor.update(cx, |e, cx| e.set_generated_body(body, window, cx)),
                    None => self.notify_user("В схеме нет тела запроса", cx),
                }
            }
            RequestEditorEvent::Save => self.save_request(window, cx),
        }
    }

    // ---- sending -------------------------------------------------------------

    /// Send the active request; production + destructive asks first.
    pub fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(request) = self.active().cloned() else {
            return;
        };
        if request.is_ws() {
            // ⌘↵ sends the composed message, or connects first.
            if self.ws_view.read(cx).is_live(&request.id) {
                self.ws_view.update(cx, |v, cx| v.send_message(cx));
            } else {
                self.ws_connect(cx);
            }
            return;
        }
        let production = self.env().is_some_and(|e| e.production);
        if production && is_destructive(request.method) && !self.confirm_open {
            self.confirm_open = true;
            cx.notify();
            return;
        }
        self.confirm_open = false;
        self.dispatch(window, cx);
    }

    /// Send without the production check (after confirmation). The whole
    /// pipeline (scripts, variables, HTTP) runs in `core::runner` off-thread.
    pub fn dispatch(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.active_tab else { return };
        self.confirm_open = false;
        let request = self.tabs[ix].request.clone();
        let collection = request
            .collection_id
            .as_ref()
            .and_then(|id| self.project.collections.iter().find(|c| &c.id == id));
        let collection_vars = collection.map(|c| c.vars.clone()).unwrap_or_default();
        let env = self.env().cloned();
        let mut settings = self.project.settings.clone();
        if let Some(follow) = request.follow_redirects {
            settings.follow_redirects = follow;
        }
        if let Some(limit) = request.max_redirects {
            settings.max_redirects = limit;
        }
        let cookie_scope = settings
            .cookies_enabled
            .then(|| match collection {
                Some(c) => c.cookies_enabled.then(|| c.id.clone()),
                None => Some(http::UNSAVED_COOKIE_SCOPE.to_string()),
            })
            .flatten();

        // Resolve up front for the loading host; without a pre-request script
        // an unresolvable URL fails right away, as before.
        let preview = vars::resolve(
            &request,
            runner::effective_env(env.as_ref(), &collection_vars).as_ref(),
        );
        let has_pre_script = !request.scripts.pre_request.trim().is_empty();
        let tab = &mut self.tabs[ix];
        if let Some((cancel, _)) = tab.inflight.take() {
            cancel.cancel();
        }
        tab.script_report = None;
        if let (Err(e), false) = (&preview, has_pre_script) {
            tab.response = ResponseState::Failed(e.clone());
            let state = tab.response.clone();
            self.response.update(cx, |r, cx| r.set_state(state, cx));
            cx.notify();
            return;
        }
        let host = preview.as_ref().map(|r| host_of(&r.url)).unwrap_or_default();
        tab.response = ResponseState::Loading { host };
        tab.last_url = preview.ok().map(|r| r.url);
        let loading = tab.response.clone();
        let base_url = tab.last_url.clone();

        let cancel = http::CancelToken::new();
        let input = runner::RunInput {
            request: request.clone(),
            env,
            collection_vars,
            settings,
            cancel: cancel.clone(),
            sessions: Some(self.http_sessions.clone()),
            cookie_scope,
        };
        let request_id = request.id.clone();
        let task = cx.spawn(async move |this, cx| {
            let output = cx.background_spawn(async move { runner::run(input) }).await;
            this.update(cx, |this, cx| this.finish(&request_id, output, cx)).ok();
        });
        self.tabs[ix].inflight = Some((cancel, task));
        self.response.update(cx, |r, cx| {
            r.set_base_url(base_url, cx);
            r.set_state(loading, cx);
            r.set_script_report(None, cx);
        });
        cx.notify();
    }

    /// Persists variable changes made by request scripts.
    fn apply_script_changes(&mut self, output: &runner::RunOutput, cx: &mut Context<Self>) {
        if !output.env_changes.is_empty() {
            let active = self.project.active_env;
            if let Some(env) = self.project.environments.get_mut(active) {
                runner::apply_changes(&mut env.vars, &output.env_changes);
                // Rebuilt on the next render (needs a window for its inputs).
                self.env_panel_stale = true;
                cx.notify();
            }
        }
        if !output.collection_changes.is_empty() {
            if let Some(collection) = output
                .sent
                .collection_id
                .as_ref()
                .and_then(|id| self.project.collections.iter_mut().find(|c| &c.id == id))
            {
                runner::apply_changes(&mut collection.vars, &output.collection_changes);
            }
        }
    }

    fn finish(&mut self, request_id: &str, output: runner::RunOutput, cx: &mut Context<Self>) {
        self.apply_script_changes(&output, cx);
        let runner::RunOutput { sent, resolved_url, result, report, missing_vars, .. } = output;
        if !missing_vars.is_empty() {
            self.notify_user(missing_vars_notice(&missing_vars), cx);
        }
        let (status, duration_ms) = match &result {
            Ok(r) => (Some(r.status), Some(r.timings.total.as_millis() as u64)),
            Err(_) => (None, None),
        };
        let state = match result {
            Ok(response) => {
                let (schema_label, mismatches) = self.check_schema(&sent, &response);
                ResponseState::Done { response, schema_label, mismatches }
            }
            Err(e) => ResponseState::Failed(e),
        };

        let env_name = self.env().map(|e| e.name.clone()).unwrap_or_default();
        store::push_history(
            &mut self.project,
            HistoryEntry { id: new_id(), at: now_secs(), env: env_name, request: sent, status, duration_ms },
        );
        let history = self.project.history.clone();
        self.history_panel.update(cx, |p, cx| p.set_entries(history, cx));

        let report = (!report.is_empty()).then_some(report);
        if let Some(ix) = self.tabs.iter().position(|t| t.request.id == request_id) {
            let tab = &mut self.tabs[ix];
            tab.inflight = None;
            tab.response = state.clone();
            tab.script_report = report;
            if resolved_url.is_some() {
                tab.last_url = resolved_url;
            }
            if self.active_tab == Some(ix) {
                let base_url = self.tabs[ix].last_url.clone();
                let report = self.tabs[ix].script_report.clone();
                let link = self.schema_link_for(&self.tabs[ix].request);
                self.response.update(cx, |r, cx| {
                    r.set_schema_link(link, cx);
                    r.set_base_url(base_url, cx);
                    r.set_state(state, cx);
                    r.set_script_report(report, cx);
                });
            }
        }
        self.schedule_save(cx);
        cx.notify();
    }

    fn check_schema(
        &self,
        sent: &RequestSpec,
        response: &crate::core::ResponseData,
    ) -> (Option<String>, Vec<crate::core::Mismatch>) {
        let (Some(spec), Some(spec_ref)) = (self.spec_for_request(sent), &sent.spec_ref) else {
            return (None, Vec::new());
        };
        let label = spec.response_schema_label(spec_ref, response.status);
        let mismatches = serde_json::from_slice::<serde_json::Value>(&response.body)
            .ok()
            .and_then(|body| spec.validate(spec_ref, response.status, &body))
            .unwrap_or_default();
        (label, mismatches)
    }

    // ---- websocket -----------------------------------------------------------

    /// Connects the active WebSocket tab (variables, headers and auth resolved
    /// like an HTTP request).
    pub fn ws_connect(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.active().filter(|r| r.is_ws()).cloned() else {
            return;
        };
        let env = self.request_env(&request);
        match ws::resolve(&request, env.as_ref()) {
            Ok(resolved) => {
                let settings = self.project.settings.clone();
                self.ws_view
                    .update(cx, |v, cx| v.connect(request, &resolved, &settings, cx));
            }
            Err(e) => self.notify_user(format!("Не удалось подключиться: {e}"), cx),
        }
        cx.notify();
    }

    /// Connect / Disconnect button.
    pub fn ws_toggle(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.active().map(|r| r.id.clone()) else {
            return;
        };
        if self.ws_view.read(cx).is_live(&id) {
            self.ws_view.update(cx, |v, cx| v.disconnect_id(&id, cx));
        } else {
            self.ws_connect(cx);
        }
    }

    fn on_ws_event(
        &mut self,
        view: &Entity<WsView>,
        ev: &WsViewEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            WsViewEvent::Changed => {
                let spec = view.read(cx).spec().clone();
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.request.id == spec.id) {
                    tab.request.headers = spec.headers;
                    tab.request.auth = spec.auth;
                    tab.request.ws = spec.ws;
                }
                self.schedule_save(cx);
            }
            WsViewEvent::Finished { request, status, duration_ms } => {
                let env = self.env().map(|e| e.name.clone()).unwrap_or_default();
                store::push_history(
                    &mut self.project,
                    HistoryEntry {
                        id: new_id(),
                        at: now_secs(),
                        env,
                        request: request.clone(),
                        status: *status,
                        duration_ms: *duration_ms,
                    },
                );
                let history = self.project.history.clone();
                self.history_panel.update(cx, |p, cx| p.set_entries(history, cx));
                self.schedule_save(cx);
            }
        }
        cx.notify();
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        let Some(ix) = self.active_tab else { return };
        if let Some((cancel, _task)) = self.tabs[ix].inflight.take() {
            cancel.cancel();
            self.tabs[ix].response = ResponseState::Failed(SendError::Cancelled);
            let state = self.tabs[ix].response.clone();
            self.response.update(cx, |r, cx| r.set_state(state, cx));
            cx.notify();
        }
    }

    fn on_response_event(
        &mut self,
        _: &Entity<ResponseView>,
        ev: &ResponseViewEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            ResponseViewEvent::Retry => self.send(window, cx),
            ResponseViewEvent::Cancel => self.cancel(cx),
            // The schema editor is out of v1; nothing to navigate to yet.
            ResponseViewEvent::UpdateSchema | ResponseViewEvent::GoToSchema => {}
        }
    }

    pub fn copy_curl(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.active() else { return };
        let env = self.request_env(request);
        match vars::resolve(request, env.as_ref()) {
            Ok(resolved) => {
                cx.write_to_clipboard(ClipboardItem::new_string(curl::to_curl_with_source(
                    request,
                    &resolved,
                    env.as_ref(),
                )));
                self.notify_user("cURL скопирован", cx);
            }
            Err(e) => self.notify_user(format!("Не удалось: {e}"), cx),
        }
    }

    // ---- collections ---------------------------------------------------------

    fn show_collection_dialog(
        &mut self,
        mode: DialogMode,
        name: String,
        preferred_collection: Option<String>,
        preferred_folder: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project.collections.is_empty() {
            self.project
                .collections
                .push(Collection::new("Мои запросы"));
            self.sync_collection_panel(window, cx);
        }
        let collections = self.project.collections.clone();
        let owner = cx.entity();
        let confirm_owner = owner.clone();
        let on_confirm = Rc::new(
            move |mode, name, collection_id, folder, window: &mut Window, cx: &mut App| {
                confirm_owner.update(cx, |this, cx| {
                    this.apply_collection_dialog(mode, name, collection_id, folder, window, cx);
                    this.collection_dialog = None;
                    window.focus(&this.focus, cx);
                    cx.notify();
                });
            },
        );
        let on_dismiss = Rc::new(move |window: &mut Window, cx: &mut App| {
            owner.update(cx, |this, cx| {
                this.collection_dialog = None;
                window.focus(&this.focus, cx);
                cx.notify();
            });
        });
        let dialog = cx.new(|cx| {
            CollectionDialog::new(
                mode,
                name,
                collections,
                preferred_collection,
                preferred_folder,
                on_confirm,
                on_dismiss,
                window,
                cx,
            )
        });
        dialog.update(cx, |d, cx| d.focus(window, cx));
        self.collection_dialog = Some(dialog);
        cx.notify();
    }

    fn apply_collection_dialog(
        &mut self,
        mode: DialogMode,
        name: String,
        collection_id: Option<String>,
        folder: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match mode {
            DialogMode::SaveRequest => {
                let Some(target) = collection_id
                    .and_then(|id| self.project.collections.iter().position(|c| c.id == id))
                else {
                    return;
                };
                let Some(mut request) = self.active().cloned() else {
                    return;
                };
                request.name = name;
                request.folder = folder;
                request.collection_id = Some(self.project.collections[target].id.clone());
                if self.project.collections[target].source.is_none() {
                    request.spec_ref = None;
                }
                if let Some(tab) = self.active_mut() {
                    tab.request = request.clone();
                }
                self.project.collections[target].requests.push(request);
                self.notify_user("Запрос сохранён", cx);
            }
            DialogMode::CreateFolder { collection_id } => {
                if let Some(col) = self
                    .project
                    .collections
                    .iter_mut()
                    .find(|c| c.id == collection_id)
                {
                    let path = name.trim_matches('/').trim().to_string();
                    if !path.is_empty() && !col.folders.contains(&path) {
                        col.folders.push(path);
                    }
                }
            }
            DialogMode::RenameFolder { collection_id, old } => {
                if let Some(col) = self
                    .project
                    .collections
                    .iter_mut()
                    .find(|c| c.id == collection_id)
                {
                    for path in &mut col.folders {
                        if path == &old {
                            *path = name.clone();
                        } else if path.starts_with(&format!("{old} / ")) {
                            *path = path.replacen(&old, &name, 1);
                        }
                    }
                    for request in &mut col.requests {
                        if let Some(path) = &mut request.folder {
                            if path == &old {
                                *path = name.clone();
                            } else if path.starts_with(&format!("{old} / ")) {
                                *path = path.replacen(&old, &name, 1);
                            }
                        }
                    }
                    for tab in &mut self.tabs {
                        if tab.request.collection_id.as_deref() == Some(&collection_id) {
                            if let Some(path) = &mut tab.request.folder {
                                if path == &old {
                                    *path = name.clone();
                                } else if path.starts_with(&format!("{old} / ")) {
                                    *path = path.replacen(&old, &name, 1);
                                }
                            }
                        }
                    }
                }
            }
            DialogMode::RenameCollection { collection_id } => {
                if let Some(col) = self
                    .project
                    .collections
                    .iter_mut()
                    .find(|c| c.id == collection_id)
                {
                    col.name = name;
                }
            }
            DialogMode::RenameRequest { request_id } => {
                for col in &mut self.project.collections {
                    if let Some(r) = col.requests.iter_mut().find(|r| r.id == request_id) {
                        r.name = name.clone();
                    }
                }
                for tab in &mut self.tabs {
                    if tab.request.id == request_id {
                        tab.request.name = name.clone();
                    }
                }
            }
        }
        self.sync_collection_panel(window, cx);
        self.schedule_save(cx);
        cx.notify();
    }

    /// Saved requests update in place; first save asks for a collection, folder and name.
    pub fn save_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut request) = self.active().cloned() else {
            return;
        };
        let existing = self
            .project
            .collections
            .iter_mut()
            .find_map(|c| c.requests.iter_mut().find(|r| r.id == request.id));
        if let Some(slot) = existing {
            if request.name.trim().is_empty() || request.name == "Untitled" {
                request.name = request_name_from_url(&request.url);
            }
            *slot = request.clone();
            if let Some(tab) = self.active_mut() {
                tab.request.name = request.name;
            }
            self.sync_collection_panel(window, cx);
            self.schedule_save(cx);
            self.notify_user("Запрос сохранён", cx);
        } else {
            let name = if request.name.trim().is_empty() || request.name == "Untitled" {
                request_name_from_url(&request.url)
            } else {
                request.name.clone()
            };
            self.show_collection_dialog(
                DialogMode::SaveRequest,
                name,
                request.collection_id,
                request.folder,
                window,
                cx,
            );
        }
    }

    fn move_saved_request(
        &mut self,
        request_id: &str,
        collection_id: &str,
        folder: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self
            .project
            .collections
            .iter()
            .position(|c| c.requests.iter().any(|r| r.id == request_id))
        else {
            return;
        };
        let Some(target) = self
            .project
            .collections
            .iter()
            .position(|c| c.id == collection_id)
        else {
            return;
        };
        let index = self.project.collections[source]
            .requests
            .iter()
            .position(|r| r.id == request_id)
            .unwrap();
        let mut request = self.project.collections[source].requests.remove(index);
        request.collection_id = Some(collection_id.to_string());
        request.folder = folder;
        if source != target {
            request.spec_ref = None;
        }
        for tab in &mut self.tabs {
            if tab.request.id == request_id {
                tab.request.collection_id = request.collection_id.clone();
                tab.request.folder = request.folder.clone();
                tab.request.spec_ref = request.spec_ref.clone();
            }
        }
        self.project.collections[target].requests.push(request);
        self.sync_collection_panel(window, cx);
        self.schedule_save(cx);
        self.notify_user("Запрос перемещён", cx);
    }

    pub fn delete_saved(&mut self, col: usize, idx: usize, cx: &mut Context<Self>) {
        if let Some(c) = self.project.collections.get_mut(col) {
            if idx < c.requests.len() {
                c.requests.remove(idx);
                self.schedule_save(cx);
                cx.notify();
            }
        }
    }

    fn sync_collection_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let collections = self.project.collections.clone();
        self.collection_panel.update(cx, |panel, cx| {
            panel.set_collections(collections, window, cx)
        });
    }

    fn on_collection_panel_event(
        &mut self,
        _: &Entity<CollectionPanel>,
        ev: &CollectionPanelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            CollectionPanelEvent::Changed(collections) => {
                for old in &self.project.collections {
                    if !collections
                        .iter()
                        .any(|current| current.id == old.id && current.cookies_enabled)
                    {
                        self.http_sessions.clear(&old.id);
                    }
                }
                self.project.collections = collections.clone();
                self.linked_specs
                    .retain(|id, _| collections.iter().any(|c| &c.id == id));
                self.schedule_save(cx);
                cx.notify();
            }
            CollectionPanelEvent::Reload(id) => {
                let Some(index) = self.project.collections.iter().position(|c| &c.id == id) else {
                    return;
                };
                match self.refresh_linked_collection(index) {
                    Ok(()) => {
                        self.sync_collection_panel(window, cx);
                        self.schedule_save(cx);
                        self.notify_user("Коллекция обновлена из файла", cx);
                    }
                    Err(e) => self.notify_user(format!("Не удалось обновить коллекцию: {e}"), cx),
                }
            }
            CollectionPanelEvent::ClearCookies(id) => {
                self.http_sessions.clear(id);
                self.notify_user("Cookies коллекции очищены", cx);
            }
            CollectionPanelEvent::Delete(id) => self.delete_collection(id, window, cx),
        }
    }

    /// Removes a collection from the project (and so from project.json).
    ///
    /// The file it was imported from is never touched: only the reference and the
    /// in-memory parsed spec are dropped. Open tabs of the collection stay open but
    /// are detached (no collection/folder), so unsaved edits are not lost.
    pub fn delete_collection(
        &mut self,
        collection_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.collection_delete_confirm = None;
        let Some(index) = self
            .project
            .collections
            .iter()
            .position(|c| c.id == collection_id)
        else {
            return;
        };
        let removed = self.project.collections.remove(index);
        self.linked_specs.remove(collection_id);
        self.http_sessions.clear(collection_id);
        let key = format!("col:{collection_id}");
        let nested = format!("{key}:");
        self.collapsed
            .retain(|k| k != &key && !k.starts_with(&nested));
        let removed_requests: HashSet<&str> =
            removed.requests.iter().map(|r| r.id.as_str()).collect();
        if match &self.collection_menu {
            Some(SidebarMenu::Collection(id)) | Some(SidebarMenu::Folder(id, _)) => {
                id == collection_id
            }
            Some(SidebarMenu::Request(id)) => removed_requests.contains(id.as_str()),
            None => false,
        } {
            self.collection_menu = None;
        }
        let mut active_detached = false;
        for (ix, tab) in self.tabs.iter_mut().enumerate() {
            if tab.request.collection_id.as_deref() == Some(collection_id) {
                tab.request.collection_id = None;
                tab.request.folder = None;
                if removed.source.is_some() {
                    // Its spec is gone; don't resolve the endpoint against another one.
                    tab.request.spec_ref = None;
                }
                active_detached |= self.active_tab == Some(ix);
            }
        }
        if active_detached {
            self.sync_active(window, cx);
        }
        self.sync_collection_panel(window, cx);
        self.schedule_save(cx);
        self.notify_user(format!("Коллекция «{}» удалена", removed.name), cx);
    }

    fn on_settings_panel_event(
        &mut self,
        _: &Entity<SettingsPanel>,
        ev: &SettingsPanelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SettingsPanelEvent::Changed(settings) = ev;
        if self.project.settings.cookies_enabled && !settings.cookies_enabled {
            self.http_sessions.clear_all();
        }
        let auto_update_changed = self.project.settings.auto_update != settings.auto_update;
        self.project.settings = settings.clone();
        if auto_update_changed {
            self.restart_update_timer(cx);
        }
        self.editor.update(cx, |editor, cx| {
            editor.set_redirect_defaults(settings.follow_redirects, settings.max_redirects, cx)
        });
        self.schedule_save(cx);
        cx.notify();
    }

    // ---- environments --------------------------------------------------------

    pub fn set_env(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.project.environments.len() {
            self.project.active_env = ix;
            let envs = self.project.environments.clone();
            self.env_panel
                .update(cx, |p, cx| p.set_envs(envs, ix, window, cx));
        }
        self.env_menu_open = false;
        self.confirm_open = false;
        self.schedule_save(cx);
        cx.notify();
    }

    pub fn cycle_env(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.project.environments.len().max(1);
        self.set_env((self.project.active_env + 1) % n, window, cx);
    }

    fn on_env_panel_event(
        &mut self,
        panel: &Entity<EnvPanel>,
        ev: &EnvPanelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            EnvPanelEvent::Changed => {
                self.project.environments = panel.read(cx).envs().to_vec();
                self.project.active_env = self
                    .project
                    .active_env
                    .min(self.project.environments.len().saturating_sub(1));
            }
            EnvPanelEvent::Activated(ix) => {
                self.project.environments = panel.read(cx).envs().to_vec();
                self.project.active_env = *ix;
            }
        }
        self.schedule_save(cx);
        cx.notify();
    }

    fn on_history_event(
        &mut self,
        _: &Entity<HistoryPanel>,
        ev: &HistoryPanelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            HistoryPanelEvent::Open(entry) => {
                let mut request = entry.request.clone();
                // A replay is a new tab, never the original (possibly edited) one.
                request.id = new_id();
                request.spec_ref = None;
                self.tabs.push(RequestTab::new(request));
                self.set_active(Some(self.tabs.len() - 1), window, cx);
            }
            HistoryPanelEvent::Clear => {
                self.project.history.clear();
                self.history_panel
                    .update(cx, |p, cx| p.set_entries(Vec::new(), cx));
                self.schedule_save(cx);
            }
        }
    }

    // ---- spec --------------------------------------------------------------

    pub fn load_spec(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match Spec::load(&path) {
            Ok(spec) => {
                self.notify_user(format!("Загружено: {} {}", spec.title, spec.version), cx);
                self.project.name = spec.title.clone();
                self.spec = Some(Arc::new(spec));
                self.spec_error = None;
                self.project.spec_path = Some(path);
            }
            Err(e) => {
                self.spec = None;
                self.spec_error = Some(e.clone());
                self.notify_user(format!("Ошибка спецификации: {e}"), cx);
            }
        }
        self.schedule_save(cx);
        cx.notify();
    }


    pub fn prompt_open_spec(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Импортировать OpenAPI, Swagger или Postman".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                if let Some(path) = paths.into_iter().next() {
                    this.update_in(cx, |this, window, cx| this.import_file(path, window, cx))
                        .ok();
                }
            }
        })
        .detach();
    }

    pub fn import_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                self.notify_user(format!("Не удалось открыть файл: {e}"), cx);
                return;
            }
        };
        let is_postman = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .is_some_and(|doc| postman::is_postman(&doc));
        if is_postman {
            match PostmanImport::parse(&text) {
                Ok(mut imported) => {
                    imported.resolve_file_paths(&path);
                    let name = imported.collection.name.clone();
                    let count = imported.collection.requests.len();
                    let first = imported.collection.requests.first().cloned();
                    self.project.collections.push(imported.collection);
                    self.sync_collection_panel(window, cx);
                    if let Some(request) = first {
                        self.open_request(request, window, cx);
                    }
                    self.schedule_save(cx);
                    self.notify_user(format!("Импортирована коллекция «{name}»: {count} запросов, {} пропущено, {} тел и {} авторизаций требуют ручной настройки", imported.skipped, imported.unsupported_bodies, imported.unsupported_auth), cx);
                }
                Err(e) => self.notify_user(e, cx),
            }
            return;
        }
        match Spec::load(&path) {
            Ok(spec) => {
                let mut collection = Collection::new(&spec.title);
                collection.source = Some(path);
                collection.vars = spec.collection_variables();
                for group in &spec.groups {
                    collection.folders.push(group.name.clone());
                    for endpoint in &group.endpoints {
                        let mut request = endpoint.to_request();
                        request.collection_id = Some(collection.id.clone());
                        request.folder = Some(group.name.clone());
                        collection.requests.push(request);
                    }
                }
                let count = collection.requests.len();
                let name = collection.name.clone();
                let first = collection.requests.first().cloned();
                self.linked_specs
                    .insert(collection.id.clone(), Arc::new(spec));
                self.project.collections.push(collection);
                self.sync_collection_panel(window, cx);
                if let Some(request) = first {
                    self.open_request(request, window, cx);
                }
                self.schedule_save(cx);
                self.notify_user(
                    format!("Импортирована коллекция «{name}»: {count} запросов"),
                    cx,
                );
            }
            Err(e) => self.notify_user(format!("Ошибка импорта: {e}"), cx),
        }
    }

    fn refresh_linked_collection(&mut self, index: usize) -> Result<(), String> {
        let collection = self
            .project
            .collections
            .get_mut(index)
            .ok_or("Коллекция не найдена")?;
        let path = collection
            .source
            .as_ref()
            .ok_or("Коллекция не связана с файлом")?;
        let spec = Spec::load(path)?;
        let old = std::mem::take(&mut collection.requests);
        for group in &spec.groups {
            if !collection.folders.contains(&group.name) {
                collection.folders.push(group.name.clone());
            }
            for endpoint in &group.endpoints {
                let mut request = endpoint.to_request();
                request.collection_id = Some(collection.id.clone());
                request.folder = Some(group.name.clone());
                if let Some(previous) = old.iter().find(|r| r.spec_ref == request.spec_ref) {
                    request.id = previous.id.clone();
                }
                collection.requests.push(request);
            }
        }
        // Refresh imported operations while keeping user-created requests in this collection.
        collection
            .requests
            .extend(old.into_iter().filter(|request| request.spec_ref.is_none()));
        for default in spec.collection_variables() {
            match collection.vars.iter_mut().find(|v| v.key == default.key) {
                Some(var) if var.key == "baseUrl" => var.value = default.value,
                Some(_) => {}
                None => collection.vars.push(default),
            }
        }
        self.linked_specs
            .insert(collection.id.clone(), Arc::new(spec));
        Ok(())
    }

    pub fn reload_spec(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut refreshed = 0;
        let mut failures = Vec::new();
        for index in 0..self.project.collections.len() {
            if self.project.collections[index].source.is_some() {
                match self.refresh_linked_collection(index) {
                    Ok(()) => refreshed += 1,
                    Err(e) => failures.push(e),
                }
            }
        }
        if let Some(path) = self.project.spec_path.clone() {
            self.load_spec(path, cx);
            refreshed += 1;
        }
        self.schedule_save(cx);
        self.sync_collection_panel(window, cx);
        self.notify_user(
            if failures.is_empty() {
                format!("Обновлено спецификаций: {refreshed}")
            } else {
                format!(
                    "Обновлено: {refreshed}; ошибок: {} ({})",
                    failures.len(),
                    failures[0]
                )
            },
            cx,
        );
        cx.notify();
    }

    // ---- palette -----------------------------------------------------------

    fn palette_items(&self) -> Vec<PaletteItem> {
        let mut items = Vec::new();
        for (gi, g) in self.spec_groups().iter().enumerate() {
            for (ei, e) in g.endpoints.iter().enumerate() {
                items.push(
                    PaletteItem::new(
                        format!("spec:{gi}:{ei}"),
                        e.name.clone(),
                        format!("{} · {}", g.name, e.path),
                    )
                    .method(e.method),
                );
            }
        }
        for (ci, c) in self.project.collections.iter().enumerate() {
            for (ri, r) in c.requests.iter().enumerate() {
                let item = PaletteItem::new(
                    format!("saved:{ci}:{ri}"),
                    if r.is_ws() { format!("WS · {}", r.name) } else { r.name.clone() },
                    format!("{} · {}", c.name, r.url),
                );
                items.push(if r.is_ws() { item } else { item.method(r.method) });
            }
        }
        let actions: [(&str, &str, &str); 12] = [
            (
                "action:new-tab",
                "Новая вкладка",
                shortcut_hint("⌘T", "Ctrl+T"),
            ),
            ("action:new-ws", "Новый WebSocket", ""),
            (
                "action:send",
                "Отправить запрос",
                shortcut_hint("⌘↵", "Ctrl+Enter"),
            ),
            (
                "action:save",
                "Сохранить запрос",
                shortcut_hint("⌘S", "Ctrl+S"),
            ),
            (
                "action:copy-curl",
                "Скопировать как cURL",
                shortcut_hint("⌘⇧C", "Ctrl+Shift+C"),
            ),
            (
                "action:open-spec",
                "Импортировать OpenAPI, Swagger или Postman…",
                shortcut_hint("⌘O", "Ctrl+O"),
            ),
            (
                "action:theme",
                "Переключить тему",
                shortcut_hint("⌘⇧L", "Ctrl+Shift+L"),
            ),
            (
                "action:sidebar",
                "Показать/скрыть sidebar",
                shortcut_hint("⌘B", "Ctrl+B"),
            ),
            (
                "action:envs",
                "Редактировать окружения",
                shortcut_hint("⌘2", "Ctrl+2"),
            ),
            (
                "action:collections",
                "Управлять коллекциями",
                shortcut_hint("⌘4", "Ctrl+4"),
            ),
            ("action:settings", "Настройки", ""),
            (
                "action:history",
                "История запросов",
                shortcut_hint("⌘3", "Ctrl+3"),
            ),
        ];
        for (id, title, kbd) in actions {
            items.push(PaletteItem::new(id, title, "Команда").kbd(kbd));
        }
        for (i, e) in self.project.environments.iter().enumerate() {
            items.push(PaletteItem::new(
                format!("env:{i}"),
                format!("Окружение: {}", e.name),
                e.base_url().to_string(),
            ));
        }
        items
    }

    pub fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.palette_items();
        let palette = cx.new(|cx| CommandPalette::new(items, window, cx));
        let sub = cx.subscribe_in(
            &palette,
            window,
            |this, _, ev: &PaletteEvent, window, cx| {
                let picked = match ev {
                    PaletteEvent::Picked(id) => Some(id.clone()),
                    PaletteEvent::Dismissed => None,
                };
                this.close_palette(window, cx);
                if let Some(id) = picked {
                    this.run_palette_item(&id, window, cx);
                }
            },
        );
        self._subscriptions.push(sub);
        palette.update(cx, |p, cx| p.focus(window, cx));
        self.palette = Some(palette);
        cx.notify();
    }

    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn run_palette_item(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let parts: Vec<&str> = id.split(':').collect();
        let num = |i: usize| parts.get(i).and_then(|s| s.parse::<usize>().ok());
        match parts.as_slice() {
            ["spec", ..] => {
                if let (Some(g), Some(e)) = (num(1), num(2)) {
                    self.open_spec_endpoint(g, e, window, cx);
                }
            }
            ["saved", ..] => {
                if let (Some(c), Some(r)) = (num(1), num(2)) {
                    self.open_saved(c, r, window, cx);
                }
            }
            ["env", ..] => {
                if let Some(i) = num(1) {
                    self.set_env(i, window, cx);
                }
            }
            ["action", "new-tab"] => self.new_tab(window, cx),
            ["action", "new-ws"] => self.new_ws_tab(window, cx),
            ["action", "send"] => self.send(window, cx),
            ["action", "save"] => self.save_request(window, cx),
            ["action", "copy-curl"] => self.copy_curl(cx),
            ["action", "open-spec"] => self.prompt_open_spec(cx),
            ["action", "theme"] => Palette::toggle(cx),
            ["action", "sidebar"] => self.sidebar_visible = !self.sidebar_visible,
            ["action", "envs"] => self.set_rail(RailMode::Environments, window, cx),
            ["action", "collections"] => self.set_rail(RailMode::Collections, window, cx),
            ["action", "settings"] => self.set_rail(RailMode::Settings, window, cx),
            ["action", "history"] => self.set_rail(RailMode::History, window, cx),
            _ => {}
        }
        cx.notify();
    }

    /// Switching rails can unmount the focused input (sidebar search), so
    /// focus goes back to the workspace to keep shortcuts working.
    /// "Создать окружение" in the titlebar picker: adds an environment and opens
    /// it on the Environments page.
    pub fn create_environment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.env_menu_open = false;
        let envs = self.project.environments.clone();
        let active = self.project.active_env;
        self.env_panel_stale = false;
        self.env_panel.update(cx, |p, cx| {
            p.set_envs(envs, active, window, cx);
            p.add_env(window, cx);
        });
        self.set_rail(RailMode::Environments, window, cx);
    }

    pub fn set_rail(&mut self, mode: RailMode, window: &mut Window, cx: &mut Context<Self>) {
        if mode == RailMode::Settings {
            let settings = self.project.settings.clone();
            self.settings_panel
                .update(cx, |panel, cx| panel.set_settings(settings, window, cx));
        }
        if mode == RailMode::Collections {
            self.sync_collection_panel(window, cx);
        }
        self.rail = mode;
        window.focus(&self.focus, cx);
        cx.notify();
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if window.focused(cx).is_none() {
            window.focus(&self.focus, cx);
        }
        let sidebar_key = self.sidebar_key(cx);
        if self.sidebar_key != Some(sidebar_key) {
            self.sidebar_key = Some(sidebar_key);
            self.sidebar_view.update(cx, |_, cx| cx.notify());
        }
        if std::mem::take(&mut self.env_panel_stale) {
            let envs = self.project.environments.clone();
            let active = self.project.active_env;
            self.env_panel.update(cx, |p, cx| p.set_envs(envs, active, window, cx));
        }
        if std::mem::take(&mut self.external_stale) {
            self.reload_external_views(window, cx);
        }
        // The Browser response mode is a native view drawn above GPUI: hide it
        // whenever GPUI content overlaps it or the response pane is off screen.
        let browser_covered = self.palette.is_some()
            || self.env_menu_open
            || self.method_menu_open
            || self.new_tab_menu_open
            || self.confirm_open
            || self.update_popover_open
            || self.request_split_dragging
            || self.rail == RailMode::Environments
            || self.rail == RailMode::Collections
            || self.rail == RailMode::Settings
            || self.active_tab.is_none();
        self.response
            .update(cx, |r, cx| r.set_browser_suppressed(browser_covered, cx));
        let settings_visible = self.rail == RailMode::Settings;
        self.settings_panel
            .update(cx, |p, cx| p.set_visible(settings_visible, cx));
        let p = *cx.palette();
        let content = match self.rail {
            RailMode::Environments => div()
                .flex()
                .flex_1()
                .min_w_0()
                .child(self.env_panel.clone())
                .into_any_element(),
            RailMode::Collections => div()
                .flex()
                .flex_1()
                .min_w_0()
                .child(self.collection_panel.clone())
                .into_any_element(),
            RailMode::Settings => div()
                .flex()
                .flex_1()
                .min_w_0()
                .child(self.settings_panel.clone())
                .into_any_element(),
            _ => div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(self.render_tabs(cx))
                .child(self.render_request_line(cx))
                .child(self.render_workspace_body(window, cx))
                .into_any_element(),
        };

        div()
            .id("workspace")
            .relative()
            .track_focus(&self.focus)
            // Any click GPUI receives happened outside the native Browser page:
            // take the keyboard back from WebKit (capture phase, before children).
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                this.response.read(cx).release_browser_keyboard();
            }))
            .on_mouse_move(cx.listener(|this, event, _, cx| {
                this.move_request_split(event, cx);
                this.move_sidebar_drag(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.stop_request_split(cx);
                    this.stop_sidebar_drag(cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.stop_request_split(cx);
                    this.stop_sidebar_drag(cx);
                }),
            )
            .key_context("Workspace")
            .on_action(cx.listener(|this, _: &NewTab, window, cx| this.new_tab(window, cx)))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                if let Some(ix) = this.active_tab {
                    this.close_tab_at(ix, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ForceCloseTab, window, cx| {
                if let Some(ix) = this.active_tab {
                    this.force_close_tab_at(ix, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.cycle_tab(1, window, cx)))
            .on_action(cx.listener(|this, _: &PrevTab, window, cx| this.cycle_tab(-1, window, cx)))
            .on_action(cx.listener(|this, _: &SendRequest, window, cx| this.send(window, cx)))
            .on_action(cx.listener(|this, _: &CancelRequest, _, cx| this.cancel(cx)))
            .on_action(
                cx.listener(|this, _: &SaveRequest, window, cx| this.save_request(window, cx)),
            )
            .on_action(cx.listener(|this, _: &CopyCurl, _, cx| this.copy_curl(cx)))
            .on_action(
                cx.listener(|this, _: &OpenPalette, window, cx| this.open_palette(window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenSpec, _, cx| this.prompt_open_spec(cx)))
            .on_action(cx.listener(|this, _: &ReloadSpec, window, cx| this.reload_spec(window, cx)))
            .on_action(cx.listener(|this, _: &SaveProject, _, cx| this.save_project(cx)))
            .on_action(cx.listener(|_, _: &ToggleTheme, _, cx| Palette::toggle(cx)))
            .on_action(cx.listener(|this, _: &ToggleEnv, window, cx| this.cycle_env(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleSidebar, window, cx| {
                this.sidebar_visible = !this.sidebar_visible;
                window.focus(&this.focus, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ShowRequests, window, cx| {
                this.set_rail(RailMode::Requests, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowEnvs, window, cx| {
                this.set_rail(RailMode::Environments, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowCollections, window, cx| {
                this.set_rail(RailMode::Collections, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowHistory, window, cx| {
                this.set_rail(RailMode::History, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowSettings, window, cx| {
                this.set_rail(RailMode::Settings, window, cx)
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(p.bg)
            .text_color(p.text)
            .text_size(px(12.5))
            .child(self.render_titlebar(cx))
            .when_some(self.app_menu_bar.clone(), |d, menu_bar| {
                d.child(
                    div()
                        .id("window-menu-row")
                        .flex_none()
                        .h(px(32.))
                        .min_w_0()
                        .px(px(8.))
                        .border_b_1()
                        .border_color(p.border)
                        .bg(p.surface)
                        .child(menu_bar)
                        .test_support(),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_rail(cx))
                    .when(
                        self.sidebar_visible
                            && self.rail != RailMode::Environments
                            && self.rail != RailMode::Collections
                            && self.rail != RailMode::Settings,
                        |d| {
                            d.child(
                                self.sidebar_view.clone().cached(
                                    StyleRefinement::default()
                                        .flex_none()
                                        .w(px(self.sidebar_width)),
                                ),
                            )
                            .child(
                                crate::ui::PaneSplitter::new("sidebar-splitter")
                                    .active(self.sidebar_drag.is_some())
                                    .on_drag_start(cx.listener(|this, event, _, cx| {
                                        this.start_sidebar_drag(event, cx)
                                    })),
                            )
                        },
                    )
                    .child(content),
            )
            .child(self.render_status_bar(cx))
            .when_some(self.palette.clone(), |d, palette| d.child(palette))
            .when_some(self.close_confirm, |d, ix| d.child(self.render_close_confirm(ix, cx)))
            .when_some(self.collection_dialog.clone(), |d, dialog| d.child(dialog))
    }
}
