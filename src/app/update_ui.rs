//! Self-update UI: the rail button above Settings with a status indicator,
//! its popover (release notes, download progress, install) and the periodic
//! check. All network and file work runs on the background executor through
//! [`UpdateHooks`], which tests replace with fakes.

use crate::ui::ScrollY as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui_kit::base::{SelectableText, TextSelectionLayer};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{Workspace, now_secs};
use crate::core::Settings;
use crate::core::http::{CancelToken, Progress, format_size};
use crate::core::update::{self, ReleaseInfo};
use crate::theme::ActivePalette;
use crate::ui::{Button, IconName, Placement, Popover, Spinner, Tooltip, icon};

/// First automatic check after start.
pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(5);
/// Interval between automatic checks.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const PROGRESS_POLL: Duration = Duration::from_millis(150);
pub const DEV_BUILD_TIP: &str = "Обновления отключены в dev-сборке";

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateState {
    Idle,
    Checking,
    Available(ReleaseInfo),
    Downloading { release: ReleaseInfo, received: u64, total: u64 },
    Ready { release: ReleaseInfo, staged: PathBuf },
    Installing(ReleaseInfo),
    /// Unix seconds of the last successful check.
    UpToDate(u64),
    /// `release` is set when the failure happened while downloading or
    /// installing it ("Повторить" downloads again); otherwise it re-checks.
    Error { message: String, release: Option<ReleaseInfo> },
}

impl UpdateState {
    pub fn release(&self) -> Option<&ReleaseInfo> {
        match self {
            UpdateState::Available(r)
            | UpdateState::Downloading { release: r, .. }
            | UpdateState::Ready { release: r, .. }
            | UpdateState::Installing(r)
            | UpdateState::Error { release: Some(r), .. } => Some(r),
            _ => None,
        }
    }

    /// One-line status for the tooltip and Settings.
    pub fn status_text(&self) -> String {
        match self {
            UpdateState::Idle => "Обновления".into(),
            UpdateState::Checking => "Проверка обновлений…".into(),
            UpdateState::Available(r) => format!("Доступна версия {}", r.version),
            UpdateState::Downloading { release, received, total } => match percent(*received, *total) {
                Some(p) => format!("Загрузка {} — {p}%", release.version),
                None => format!("Загрузка {}…", release.version),
            },
            UpdateState::Ready { release, .. } => format!("Версия {} готова к установке", release.version),
            UpdateState::Installing(r) => format!("Установка {}…", r.version),
            UpdateState::UpToDate(at) => format!("У вас последняя версия · проверено {}", ago(*at)),
            UpdateState::Error { message, .. } => format!("Ошибка обновления: {message}"),
        }
    }
}

fn percent(received: u64, total: u64) -> Option<u64> {
    (total > 0).then(|| (received.min(total) * 100) / total)
}

fn ago(at: u64) -> String {
    let secs = now_secs().saturating_sub(at);
    match secs {
        0..60 => "только что".into(),
        60..3600 => format!("{} мин назад", secs / 60),
        _ => format!("{} ч назад", secs / 3600),
    }
}

pub type CheckFn = Arc<dyn Fn(&Settings) -> Result<Option<ReleaseInfo>, String> + Send + Sync>;
pub type DownloadFn =
    Arc<dyn Fn(&ReleaseInfo, &Settings, Progress, &CancelToken) -> Result<PathBuf, String> + Send + Sync>;
pub type InstallFn = Arc<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;

/// The updater's side effects. [`UpdateHooks::production`] talks to GitHub and
/// replaces the installed app; tests inject fakes.
#[derive(Clone)]
pub struct UpdateHooks {
    /// `false` in dev builds: the rail button is dimmed and nothing runs.
    pub enabled: bool,
    pub check: CheckFn,
    pub download: DownloadFn,
    /// Swaps files and starts the new version (background thread).
    pub install: InstallFn,
    /// After a successful install, on the UI thread (production: quit).
    pub on_installed: Rc<dyn Fn(&mut App)>,
    /// Why this copy can't replace itself (shown instead of «Скачать»).
    pub blocker: Option<String>,
}

impl UpdateHooks {
    pub fn production() -> Self {
        let dev = update::is_dev_build();
        Self {
            enabled: !dev,
            check: Arc::new(update::check_latest),
            download: Arc::new(|release, settings, progress, cancel| {
                update::download(release, settings, progress, cancel)
            }),
            install: Arc::new(update::install_and_restart),
            on_installed: Rc::new(|cx| cx.quit()),
            blocker: if dev { None } else { update::install_blocker() },
        }
    }

    /// Does nothing; enabled (for tests that drive the state by hand).
    pub fn inert() -> Self {
        Self {
            enabled: true,
            check: Arc::new(|_| Ok(None)),
            download: Arc::new(|_, _, _, _| Err("disabled".into())),
            install: Arc::new(|_| Err("disabled".into())),
            on_installed: Rc::new(|_| {}),
            blocker: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Indicator {
    Available,
    Downloading,
    Ready,
    Error,
}

impl Workspace {
    // ---- state -----------------------------------------------------------------

    pub fn set_update_hooks(&mut self, hooks: UpdateHooks, cx: &mut Context<Self>) {
        self.update_hooks = hooks;
        self.restart_update_timer(cx);
        self.sync_update_status(cx);
        cx.notify();
    }

    pub fn set_update_state(&mut self, state: UpdateState, cx: &mut Context<Self>) {
        self.update_state = state;
        self.sync_update_status(cx);
        cx.notify();
    }

    fn is_skipped(&self, release: &ReleaseInfo) -> bool {
        self.project.settings.skipped_version.as_deref() == Some(release.version.to_string().as_str())
    }

    fn indicator(&self) -> Option<Indicator> {
        if !self.update_hooks.enabled {
            return None;
        }
        match &self.update_state {
            UpdateState::Available(r) if !self.is_skipped(r) => Some(Indicator::Available),
            UpdateState::Downloading { .. } | UpdateState::Installing(_) => Some(Indicator::Downloading),
            UpdateState::Ready { .. } => Some(Indicator::Ready),
            UpdateState::Error { .. } => Some(Indicator::Error),
            _ => None,
        }
    }

    /// The rail button shows a dot / spinner.
    pub fn update_indicator_visible(&self) -> bool {
        self.indicator().is_some()
    }

    /// (Re)starts the automatic check: [`FIRST_CHECK_DELAY`] after start, then
    /// every [`CHECK_INTERVAL`]. Off in dev builds and when disabled in Settings.
    pub(super) fn restart_update_timer(&mut self, cx: &mut Context<Self>) {
        if !(self.update_hooks.enabled && self.project.settings.auto_update) {
            self._update_timer = None;
            return;
        }
        self._update_timer = Some(cx.spawn(async move |this, cx| {
            let mut delay = FIRST_CHECK_DELAY;
            loop {
                cx.background_executor().timer(delay).await;
                if this.update(cx, |this, cx| this.check_for_updates(false, cx)).is_err() {
                    break;
                }
                delay = CHECK_INTERVAL;
            }
        }));
    }

    pub(super) fn sync_update_status(&mut self, cx: &mut Context<Self>) {
        let text = if self.update_hooks.enabled {
            self.update_state.status_text()
        } else {
            DEV_BUILD_TIP.to_string()
        };
        let enabled = self.update_hooks.enabled;
        self.settings_panel
            .update(cx, |panel, cx| panel.set_update_status(text, enabled, cx));
    }

    /// Asks GitHub for a newer release. `manual` checks report errors and open
    /// the popover when an update is found; automatic ones fail silently.
    pub fn check_for_updates(&mut self, manual: bool, cx: &mut Context<Self>) {
        if !self.update_hooks.enabled
            || matches!(
                self.update_state,
                UpdateState::Checking
                    | UpdateState::Downloading { .. }
                    | UpdateState::Ready { .. }
                    | UpdateState::Installing(_)
            )
        {
            return;
        }
        let previous = std::mem::replace(&mut self.update_state, UpdateState::Checking);
        let check = self.update_hooks.check.clone();
        let settings = self.project.settings.clone();
        self.update_task = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { check(&settings) }).await;
            this.update(cx, |this, cx| this.finish_check(result, manual, previous, cx)).ok();
        }));
        self.sync_update_status(cx);
        cx.notify();
    }

    fn finish_check(
        &mut self,
        result: Result<Option<ReleaseInfo>, String>,
        manual: bool,
        previous: UpdateState,
        cx: &mut Context<Self>,
    ) {
        self.update_state = match result {
            Ok(Some(release)) => {
                if manual {
                    self.update_popover_open = true;
                }
                UpdateState::Available(release)
            }
            Ok(None) => UpdateState::UpToDate(now_secs()),
            Err(message) if manual => UpdateState::Error { message, release: None },
            Err(message) => {
                eprintln!("rivet: update check failed: {message}");
                match previous {
                    UpdateState::Error { .. } | UpdateState::Checking => UpdateState::Idle,
                    other => other,
                }
            }
        };
        self.sync_update_status(cx);
        cx.notify();
    }

    /// Downloads and verifies the available release; progress is polled from
    /// the background download every [`PROGRESS_POLL`].
    pub fn download_update(&mut self, cx: &mut Context<Self>) {
        let release = match &self.update_state {
            UpdateState::Available(r) | UpdateState::Error { release: Some(r), .. } => r.clone(),
            _ => return,
        };
        if !self.update_hooks.enabled || self.update_hooks.blocker.is_some() {
            return;
        }
        let received = Arc::new(AtomicU64::new(0));
        let total = Arc::new(AtomicU64::new(release.asset.size));
        let progress: Progress = {
            let (received, total) = (received.clone(), total.clone());
            Arc::new(move |r, t| {
                received.store(r, Ordering::Relaxed);
                if t > 0 {
                    total.store(t, Ordering::Relaxed);
                }
            })
        };
        let cancel = CancelToken::new();
        self.update_cancel = Some(cancel.clone());
        self.update_state = UpdateState::Downloading {
            release: release.clone(),
            received: 0,
            total: release.asset.size,
        };

        self._update_progress = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(PROGRESS_POLL).await;
                let running = this.update(cx, |this, cx| {
                    let UpdateState::Downloading { received: r, total: t, .. } = &mut this.update_state
                    else {
                        return false;
                    };
                    let (new_r, new_t) = (received.load(Ordering::Relaxed), total.load(Ordering::Relaxed));
                    if (*r, *t) != (new_r, new_t) {
                        (*r, *t) = (new_r, new_t);
                        this.sync_update_status(cx);
                        cx.notify();
                    }
                    true
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        }));

        let download = self.update_hooks.download.clone();
        let settings = self.project.settings.clone();
        self.update_task = Some(cx.spawn(async move |this, cx| {
            let job = {
                let (release, cancel) = (release.clone(), cancel.clone());
                cx.background_spawn(async move { download(&release, &settings, progress, &cancel) })
            };
            let result = job.await;
            this.update(cx, |this, cx| {
                this.update_cancel = None;
                this._update_progress = None;
                this.update_state = match result {
                    Ok(staged) => UpdateState::Ready { release, staged },
                    Err(_) if cancel.is_cancelled() => UpdateState::Available(release),
                    Err(message) => UpdateState::Error { message, release: Some(release) },
                };
                this.sync_update_status(cx);
                cx.notify();
            })
            .ok();
        }));
        self.sync_update_status(cx);
        cx.notify();
    }

    pub fn cancel_update_download(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = self.update_cancel.take() {
            cancel.cancel();
        }
        cx.notify();
    }

    /// Saves the project, swaps in the staged version and restarts.
    pub fn install_update(&mut self, cx: &mut Context<Self>) {
        let UpdateState::Ready { release, staged } = self.update_state.clone() else {
            return;
        };
        self.update_state = UpdateState::Installing(release.clone());
        self.save_now();
        let install = self.update_hooks.install.clone();
        self.update_task = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { install(&staged) }).await;
            this.update(cx, |this, cx| match result {
                Ok(()) => {
                    let done = this.update_hooks.on_installed.clone();
                    done(cx);
                }
                Err(message) => {
                    this.update_state = UpdateState::Error { message, release: Some(release) };
                    this.sync_update_status(cx);
                    cx.notify();
                }
            })
            .ok();
        }));
        self.sync_update_status(cx);
        cx.notify();
    }

    /// «Пропустить эту версию»: no indicator for it any more.
    pub fn skip_update(&mut self, cx: &mut Context<Self>) {
        let Some(release) = self.update_state.release() else {
            return;
        };
        let version = release.version.to_string();
        self.project.settings.skipped_version = Some(version.clone());
        self.settings_panel
            .update(cx, |panel, _| panel.set_skipped_version(Some(version)));
        self.update_popover_open = false;
        self.schedule_save(cx);
        cx.notify();
    }

    fn retry_update(&mut self, cx: &mut Context<Self>) {
        if matches!(self.update_state, UpdateState::Error { release: Some(_), .. }) {
            self.download_update(cx);
        } else {
            self.update_state = UpdateState::Idle;
            self.check_for_updates(true, cx);
        }
    }

    // ---- rendering ---------------------------------------------------------------

    /// Rail button (above Settings) with its popover.
    pub(super) fn render_update_rail_item(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *cx.palette();
        let enabled = self.update_hooks.enabled;
        let tip = if enabled { self.update_state.status_text() } else { DEV_BUILD_TIP.to_string() };
        let fg = if self.update_popover_open { p.accent } else { p.text_dim };
        let indicator = self.indicator().map(|kind| {
            let dot = div().id("rail-update-dot").absolute().top(px(5.)).right(px(5.));
            let color = match kind {
                Indicator::Available => p.accent,
                Indicator::Ready => p.online,
                Indicator::Error => p.danger,
                Indicator::Downloading => {
                    return dot
                        .top(px(2.))
                        .right(px(2.))
                        .child(Spinner::new("rail-update-spinner").size(12.).color(p.accent))
                        .test_support();
                }
            };
            dot.size(px(8.))
                .rounded_full()
                .bg(color)
                .border_1()
                .border_color(p.surface)
                .test_support()
        });
        let button = div()
            .id("rail-update")
            .aria_label(tip.clone())
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .size(px(36.))
            .rounded(px(8.))
            .when(self.update_popover_open, |s| s.bg(p.accent.opacity(0.16)))
            .when(enabled, |s| {
                s.cursor_pointer()
                    .hover(|s| s.bg(p.surface_hi))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.update_popover_open = !this.update_popover_open;
                        cx.notify();
                    }))
            })
            .when(!enabled, |s| s.opacity(0.45))
            .child(icon(IconName::CircleArrowDown, 18., fg))
            .children(indicator)
            .tooltip(Tooltip::new(tip).build())
            .test_support();

        let view = cx.entity().downgrade();
        Popover::new("rail-update-popover")
            .open(enabled && self.update_popover_open)
            .placement(Placement::RightEnd)
            .gap(10.)
            .width(400.)
            .trigger(button)
            .child(div().id("update-popover").child(self.render_update_popover(cx)).test_support())
            .on_dismiss(move |_, cx| {
                view.update(cx, |this, cx| {
                    this.update_popover_open = false;
                    cx.notify();
                })
                .ok();
            })
            .into_any_element()
    }

    fn render_update_popover(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *cx.palette();
        let current = update::current_version();
        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(14.))
            .text_size(px(12.5))
            .child(TextSelectionLayer);

        let Some(release) = self.update_state.release().cloned() else {
            // Nothing to install: up to date, checking, or a failed check.
            let (title, detail) = match &self.update_state {
                UpdateState::Checking => ("Проверка обновлений…".to_string(), String::new()),
                UpdateState::Error { message, .. } => ("Не удалось проверить обновления".to_string(), message.clone()),
                UpdateState::UpToDate(at) => (format!("У вас последняя версия {current}"), format!("Проверено {}", ago(*at))),
                _ => (format!("Rivet {current}"), "Обновления ещё не проверялись".to_string()),
            };
            let checking = self.update_state == UpdateState::Checking;
            let failed = matches!(self.update_state, UpdateState::Error { .. });
            return body
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .when(checking, |d| d.child(Spinner::new("update-checking")))
                        .child(
                            div()
                                .id("update-title")
                                .aria_label(title.clone())
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title)
                                .test_support(),
                        ),
                )
                .when(!detail.is_empty(), |d| {
                    d.child(
                        div()
                            .id("update-error")
                            .text_color(if failed { p.danger } else { p.text_dim })
                            .child(SelectableText::new("update-detail", detail)),
                    )
                })
                .child(
                    div().flex().gap(px(8.)).child(
                        Button::new("update-check-now", if failed { "Повторить" } else { "Проверить снова" })
                            .disabled(checking)
                            .on_click(cx.listener(|this, _, _, cx| this.retry_update(cx))),
                    ),
                )
                .into_any_element();
        };

        let title = format!("Rivet {} доступен", release.version);
        let published = update::format_date(&release.published_at);
        let meta = if published.is_empty() {
            format!("Текущая версия {current}")
        } else {
            format!("Текущая версия {current} · опубликовано {published}")
        };
        let notes = if release.notes.trim().is_empty() {
            "Описание изменений не указано.".to_string()
        } else {
            release.notes.trim().replace("\r\n", "\n")
        };
        let release_page = {
            let url = release.html_url.clone();
            Button::new("update-release-page", "Открыть страницу релиза")
                .ghost()
                .icon(IconName::ExternalLink)
                .on_click(move |_, _, cx| cx.open_url(&url))
        };

        let actions: AnyElement = match &self.update_state {
            UpdateState::Available(_) => div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .when_some(self.update_hooks.blocker.clone(), |d, reason| {
                    d.child(
                        div()
                            .id("update-blocker")
                            .text_size(px(11.5))
                            .text_color(p.warn)
                            .child(reason),
                    )
                })
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(8.))
                        .child(
                            Button::new(
                                "update-download",
                                format!("Скачать · {}", format_size(release.asset.size as usize)),
                            )
                            .primary()
                            .disabled(self.update_hooks.blocker.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.download_update(cx))),
                        )
                        .child(
                            Button::new("update-skip", "Пропустить эту версию")
                                .on_click(cx.listener(|this, _, _, cx| this.skip_update(cx))),
                        ),
                )
                .child(release_page)
                .into_any_element(),
            UpdateState::Downloading { received, total, .. } => {
                let fraction = if *total > 0 { (*received as f32 / *total as f32).clamp(0., 1.) } else { 0. };
                let label = if *total > 0 {
                    format!("{} из {}", format_size(*received as usize), format_size(*total as usize))
                } else {
                    format!("{} загружено", format_size(*received as usize))
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .id("update-progress")
                            .aria_label(label.clone())
                            .h(px(6.))
                            .w_full()
                            .rounded_full()
                            .bg(p.surface_hi)
                            .overflow_hidden()
                            .child(div().h_full().w(relative(fraction)).rounded_full().bg(p.accent))
                            .test_support(),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().text_color(p.text_dim).child(label))
                            .child(
                                Button::new("update-cancel", "Отменить")
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| this.cancel_update_download(cx))),
                            ),
                    )
                    .into_any_element()
            }
            UpdateState::Ready { .. } => div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .text_color(p.online)
                        .child("Загружено и проверено (SHA-256). Rivet перезапустится, проект сохранится."),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(
                            Button::new("update-install", "Перезапустить и обновить")
                                .primary()
                                .on_click(cx.listener(|this, _, _, cx| this.install_update(cx))),
                        )
                        .child(Button::new("update-later", "Позже").on_click(cx.listener(|this, _, _, cx| {
                            this.update_popover_open = false;
                            cx.notify();
                        }))),
                )
                .into_any_element(),
            UpdateState::Installing(_) => div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(Spinner::new("update-installing"))
                .child("Установка и перезапуск…")
                .into_any_element(),
            UpdateState::Error { message, .. } => div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .id("update-error")
                        .text_color(p.danger)
                        .child(SelectableText::new("update-error-text", message.clone())),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(
                            Button::new("update-retry", "Повторить")
                                .primary()
                                .on_click(cx.listener(|this, _, _, cx| this.retry_update(cx))),
                        )
                        .child(release_page),
                )
                .into_any_element(),
            _ => div().into_any_element(),
        };

        body.child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .id("update-title")
                        .aria_label(title.clone())
                        .text_size(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title)
                        .test_support(),
                )
                .child(div().text_size(px(11.5)).text_color(p.text_dim).child(meta)),
        )
        .child(
            div()
                .id("update-notes")
                .aria_label(notes.clone())
                .max_h(px(240.))
                .scroll_y(&self.scrolls.get("update-notes"))
                .p(px(10.))
                .rounded(px(8.))
                .bg(p.bg)
                .border_1()
                .border_color(p.border)
                .child(SelectableText::new("update-notes-text", notes))
                .test_support(),
        )
        .child(actions)
        .into_any_element()
    }
}
