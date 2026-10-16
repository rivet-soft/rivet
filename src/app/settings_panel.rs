//! Application settings, separate from request and environment editors.
//! Includes Settings → MCP: permissions of `rivet mcp`, one-click connection
//! of AI assistants and the server's activity log.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::base::{SelectableText, TextSelectionLayer};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::core::mcp_activity::{self, ActivityEntry};
use crate::core::mcp_clients::{self, McpClient, McpClientPaths, McpStatus, RivetCommand};
use crate::core::{HttpVersionPref, McpSettings, Settings};
use crate::theme::{ActivePalette, MONO};
use crate::ui::{Button, ConfirmPopover, Pill, Placement, Segmented};

/// Activity entries shown in Settings → MCP.
pub const ACTIVITY_SHOWN: usize = 20;
/// How often the activity log is re-read while the panel is visible.
const ACTIVITY_POLL: Duration = Duration::from_secs(2);
const NOTICE_TTL: Duration = Duration::from_secs(4);

pub struct SettingsPanel {
    settings: Settings,
    proxy: Entity<InputState>,
    timeout: Entity<InputState>,
    redirect_limit: Entity<InputState>,
    data_path: String,
    /// `project.json`; the MCP activity log lives next to it.
    project_path: Option<PathBuf>,
    /// Client config locations (tests point them at temp files).
    pub client_paths: McpClientPaths,
    /// What the clients are told to run.
    pub command: RivetCommand,
    pub mcp_status: McpStatus,
    /// Newest first, at most [`ACTIVITY_SHOWN`].
    pub activity: Vec<ActivityEntry>,
    /// Short status line of the MCP section ("Скопировано", installer result).
    pub notice: Option<SharedString>,
    /// Client whose "Подключить" confirmation is open.
    pub confirm_install: Option<McpClient>,
    /// Client whose installer is running.
    pub installing: Option<McpClient>,
    visible: bool,
    notice_generation: u64,
    _activity_task: Option<Task<()>>,
    _status_task: Option<Task<()>>,
    _install_task: Option<Task<()>>,
    _notice_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone)]
pub enum SettingsPanelEvent {
    Changed(Settings),
}
impl EventEmitter<SettingsPanelEvent> for SettingsPanel {}

impl SettingsPanel {
    pub fn new(
        settings: Settings,
        data_path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let proxy = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("http://127.0.0.1:8080");
            input.set_value(settings.proxy.clone().unwrap_or_default(), window, cx);
            input
        });
        let timeout = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("30");
            input.set_value((settings.timeout_ms / 1000).to_string(), window, cx);
            input
        });
        let redirect_limit = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("10");
            input.set_value(settings.max_redirects.to_string(), window, cx);
            input
        });
        let subscriptions = vec![
            cx.subscribe_in(&proxy, window, |this, state, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    let value = state.read(cx).value().trim().to_string();
                    this.settings.proxy = (!value.is_empty()).then_some(value);
                    cx.emit(SettingsPanelEvent::Changed(this.settings.clone()));
                }
            }),
            cx.subscribe_in(&timeout, window, |this, state, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    if let Ok(seconds) = state.read(cx).value().trim().parse::<u64>() {
                        if (1..=3600).contains(&seconds) {
                            this.settings.timeout_ms = seconds * 1000;
                            cx.emit(SettingsPanelEvent::Changed(this.settings.clone()));
                        }
                    }
                }
            }),
            cx.subscribe_in(
                &redirect_limit,
                window,
                |this, state, ev: &InputEvent, _, cx| {
                    if let InputEvent::Change = ev {
                        if let Ok(limit) = state.read(cx).value().trim().parse::<u32>() {
                            if (1..=50).contains(&limit) {
                                this.settings.max_redirects = limit;
                                cx.emit(SettingsPanelEvent::Changed(this.settings.clone()));
                            }
                        }
                    }
                },
            ),
        ];
        Self {
            settings,
            proxy,
            timeout,
            redirect_limit,
            data_path,
            project_path: None,
            client_paths: McpClientPaths::default(),
            command: RivetCommand::current(),
            mcp_status: McpStatus::default(),
            activity: Vec::new(),
            notice: None,
            confirm_install: None,
            installing: None,
            visible: false,
            notice_generation: 0,
            _activity_task: None,
            _status_task: None,
            _install_task: None,
            _notice_task: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The project file whose sibling `mcp-activity.jsonl` is shown.
    pub fn set_project_path(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        self.project_path = path;
        self.refresh_activity(cx);
    }

    /// Overrides where client configs are read and written (tests, portable setups).
    pub fn set_client_paths(&mut self, paths: McpClientPaths, cx: &mut Context<Self>) {
        self.client_paths = paths;
        self.refresh_status(cx);
    }

    pub fn set_command(&mut self, command: RivetCommand, cx: &mut Context<Self>) {
        self.command = command;
        cx.notify();
    }

    /// Called every frame with whether the Settings screen is shown: while it
    /// is, the activity log is re-read every [`ACTIVITY_POLL`].
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if !visible {
            self._activity_task = None;
            return;
        }
        self.refresh_status(cx);
        self.refresh_activity(cx);
        self._activity_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(ACTIVITY_POLL).await;
                if this.update(cx, |this, cx| this.refresh_activity(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    /// Re-reads the activity log off the UI thread.
    pub fn refresh_activity(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project_path.clone() else {
            return;
        };
        let task = cx.background_spawn(async move {
            mcp_activity::read(&mcp_activity::activity_path(&project), ACTIVITY_SHOWN)
        });
        cx.spawn(async move |this, cx| {
            let entries = task.await;
            this.update(cx, |this, cx| {
                if this.activity != entries {
                    this.activity = entries;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Re-detects which clients have Rivet configured, off the UI thread.
    pub fn refresh_status(&mut self, cx: &mut Context<Self>) {
        let paths = self.client_paths.clone();
        let task = cx.background_spawn(async move { mcp_clients::detect_status(&paths) });
        self._status_task = Some(cx.spawn(async move |this, cx| {
            let status = task.await;
            this.update(cx, |this, cx| {
                this.mcp_status = status;
                cx.notify();
            })
            .ok();
        }));
    }

    fn set_notice(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.notice = Some(text.into());
        self.notice_generation = self.notice_generation.wrapping_add(1);
        let generation = self.notice_generation;
        self._notice_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(NOTICE_TTL).await;
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

    fn toggle_mcp(&mut self, edit: fn(&mut McpSettings), cx: &mut Context<Self>) {
        edit(&mut self.settings.mcp);
        cx.emit(SettingsPanelEvent::Changed(self.settings.clone()));
        cx.notify();
    }

    /// Copies the client's command / JSON snippet.
    pub fn copy_snippet(&mut self, client: McpClient, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(mcp_clients::snippet(client, &self.command)));
        self.set_notice("Скопировано", cx);
    }

    /// Runs the installer for `client` off the UI thread, then reports and
    /// refreshes the status.
    pub fn install(&mut self, client: McpClient, cx: &mut Context<Self>) {
        self.confirm_install = None;
        if self.installing.is_some() {
            return;
        }
        self.installing = Some(client);
        let (paths, command) = (self.client_paths.clone(), self.command.clone());
        let task = cx.background_spawn(async move { mcp_clients::install(client, &paths, &command) });
        self._install_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.installing = None;
                match result {
                    Ok(text) => this.set_notice(format!("{}: {}", client.label(), text), cx),
                    Err(e) => this.set_notice(format!("{}: ошибка — {}", client.label(), e), cx),
                }
                this.refresh_status(cx);
            })
            .ok();
        }));
        cx.notify();
    }

    pub fn clear_activity(&mut self, cx: &mut Context<Self>) {
        if let Some(project) = &self.project_path {
            if let Err(e) = mcp_activity::clear(&mcp_activity::activity_path(project)) {
                self.set_notice(format!("Не удалось очистить журнал: {e}"), cx);
            }
        }
        self.activity.clear();
        cx.notify();
    }

    pub fn set_settings(
        &mut self,
        settings: Settings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings = settings.clone();
        self.proxy.update(cx, |s, cx| {
            s.set_value(settings.proxy.unwrap_or_default(), window, cx)
        });
        self.timeout.update(cx, |s, cx| {
            s.set_value((settings.timeout_ms / 1000).to_string(), window, cx)
        });
        self.redirect_limit.update(cx, |s, cx| {
            s.set_value(settings.max_redirects.to_string(), window, cx)
        });
        cx.notify();
    }
}

fn on_off(value: bool) -> &'static str {
    if value { "Включено" } else { "Выключено" }
}

impl SettingsPanel {
    fn mcp_toggle(
        &self,
        id: &'static str,
        title: &'static str,
        help: &'static str,
        value: bool,
        disabled: bool,
        edit: fn(&mut McpSettings),
        cx: &mut Context<Self>,
    ) -> Div {
        let p = *cx.palette();
        div()
            .flex()
            .items_center()
            .gap(px(20.))
            .child(
                div()
                    .w(px(260.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(div().text_size(px(13.)).child(title))
                    .child(div().text_size(px(11.)).text_color(p.text_dim).child(help)),
            )
            .child(
                Button::new(id, on_off(value))
                    .disabled(disabled)
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_mcp(edit, cx))),
            )
    }

    fn render_client_row(&self, i: usize, client: McpClient, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = *cx.palette();
        let connected = self.mcp_status.get(client);
        let view = cx.entity().downgrade();
        let cancel_view = view.clone();
        let installing = self.installing == Some(client);
        let target = match client {
            McpClient::ClaudeCode => format!("claude mcp add --scope user rivet → {}", self.client_paths.config_path(client).display()),
            _ => self.client_paths.config_path(client).display().to_string(),
        };
        let install = Button::new(("mcp-install", i), if installing { "Подключение…" } else { "Подключить" })
            .primary()
            .disabled(self.installing.is_some())
            .on_click(cx.listener(move |this, _, _, cx| {
                this.confirm_install = Some(client);
                cx.notify();
            }));
        div()
            .id(("mcp-client", i))
            .flex()
            .items_center()
            .gap(px(12.))
            .py(px(4.))
            .child(div().w(px(130.)).text_size(px(13.)).child(client.label()))
            .child(
                div().w(px(110.)).child(if connected {
                    Pill::new("подключено").color(p.online)
                } else {
                    Pill::new("не подключено")
                }),
            )
            .child(
                Button::new(("mcp-copy", i), "Копировать")
                    .on_click(cx.listener(move |this, _, _, cx| this.copy_snippet(client, cx))),
            )
            .child(
                ConfirmPopover::new(("mcp-install-confirm", i), target)
                    .open(self.confirm_install == Some(client))
                    .placement(Placement::Below)
                    .title(format!(
                        "Добавить Rivet в конфиг {} ({})? Остальные серверы не изменятся.",
                        client.label(),
                        self.client_paths.config_path(client).display()
                    ))
                    .confirm_label("Подключить")
                    .trigger(install)
                    .on_cancel(move |_, cx| {
                        cancel_view
                            .update(cx, |this, cx| {
                                this.confirm_install = None;
                                cx.notify();
                            })
                            .ok();
                    })
                    .on_confirm(move |_, cx| {
                        view.update(cx, |this, cx| this.install(client, cx)).ok();
                    }),
            )
    }

    fn render_mcp(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = *cx.palette();
        let mcp = self.settings.mcp;
        let off = !mcp.enabled;
        let section = |title: &'static str| {
            div()
                .pt(px(6.))
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title)
        };
        let clients = McpClient::ALL
            .iter()
            .enumerate()
            .map(|(i, c)| self.render_client_row(i, *c, cx))
            .collect::<Vec<_>>();
        let activity: Vec<AnyElement> = if self.activity.is_empty() {
            vec![div()
                .text_size(px(12.))
                .text_color(p.text_dim)
                .child("Пока нет вызовов. Здесь появятся действия ассистентов через MCP.")
                .into_any_element()]
        } else {
            self.activity
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    div()
                        .id(("mcp-activity", i))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .text_size(px(12.))
                        .child(
                            div()
                                .flex_none()
                                .w(px(62.))
                                .font_family(MONO)
                                .text_color(p.text_dim)
                                .child(mcp_activity::format_time(e.at)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .w(px(14.))
                                .text_color(if e.ok { p.online } else { p.danger })
                                .child(if e.ok { "✓" } else { "✗" }),
                        )
                        .child(
                            div()
                                .flex_none()
                                .w(px(150.))
                                .font_family(MONO)
                                .child(e.tool.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(SelectableText::new(("mcp-activity-summary", i), e.summary.clone())),
                        )
                        .when_some(e.client.clone(), |d, client| {
                            d.child(div().flex_none().text_color(p.text_dim).child(client))
                        })
                        .test_support()
                        .into_any_element()
                })
                .collect()
        };
        div()
            .id("settings-mcp")
            .flex()
            .flex_col()
            .gap(px(14.))
            .pt(px(16.))
            .border_t_1()
            .border_color(p.border)
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("MCP — управление Rivet из ИИ-ассистентов"),
            )
            .child(div().max_w(px(640.)).text_size(px(12.)).text_color(p.text_dim).child(
                "Claude Code, Claude Desktop, Cursor и другие ассистенты запускают Rivet как MCP-сервер \
                 (rivet mcp) и могут смотреть коллекции, отправлять запросы и менять переменные этого проекта. \
                 Настройки применяются к следующему вызову ассистента без перезапуска.",
            ))
            .child(self.mcp_toggle(
                "settings-mcp-enabled",
                "Разрешить MCP",
                "Выключено — ассистенты получают отказ на любой вызов",
                mcp.enabled,
                false,
                |m| m.enabled = !m.enabled,
                cx,
            ))
            .child(self.mcp_toggle(
                "settings-mcp-readonly",
                "Только чтение",
                "Нельзя сохранять запросы, менять переменные и окружения, импортировать; отправки не пишут историю и переменные",
                mcp.read_only,
                off,
                |m| m.read_only = !m.read_only,
                cx,
            ))
            .child(self.mcp_toggle(
                "settings-mcp-send",
                "Разрешить отправку запросов",
                "send_request и run_collection. Production всё равно требует подтверждения",
                mcp.allow_send,
                off,
                |m| m.allow_send = !m.allow_send,
                cx,
            ))
            .child(
                self.mcp_toggle(
                    "settings-mcp-secrets",
                    "Показывать секреты ассистенту",
                    "Токены, пароли и ключи по запросу reveal_secrets",
                    mcp.reveal_secrets,
                    off,
                    |m| m.reveal_secrets = !m.reveal_secrets,
                    cx,
                ),
            )
            .when(mcp.reveal_secrets, |d| {
                d.child(
                    div()
                        .id("settings-mcp-secrets-warning")
                        .max_w(px(640.))
                        .text_size(px(11.5))
                        .text_color(p.warn)
                        .child("Секреты будут переданы ИИ-ассистенту и его провайдеру. Включайте только для доверенных клиентов.")
                        .test_support(),
                )
            })
            .child(section("Подключение"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .text_size(px(12.))
                    .child(div().flex_none().text_color(p.text_dim).child("Команда:"))
                    .child(
                        div()
                            .min_w_0()
                            .px(px(7.))
                            .py(px(3.))
                            .rounded(px(5.))
                            .bg(p.surface)
                            .font_family(MONO)
                            .text_size(px(11.5))
                            .child(SelectableText::new(
                                "mcp-exe",
                                format!("{} {}", self.command.exe_display(), self.command.args.join(" ")),
                            )),
                    ),
            )
            .children(clients)
            .when_some(self.notice.clone(), |d, notice| {
                d.child(
                    div()
                        .id("mcp-notice")
                        .max_w(px(640.))
                        .text_size(px(12.))
                        .text_color(p.accent)
                        .child(SelectableText::new("mcp-notice-text", notice))
                        .test_support(),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(section("Активность"))
                    .child(div().flex_1())
                    .child(
                        Button::new("mcp-clear-activity", "Очистить журнал")
                            .ghost()
                            .disabled(self.activity.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| this.clear_activity(cx))),
                    ),
            )
            .child(div().flex().flex_col().gap(px(5.)).children(activity))
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *cx.palette();
        let label = |title: &'static str, help: &'static str| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(div().text_size(px(13.)).child(title))
                .child(div().text_size(px(11.)).text_color(p.text_dim).child(help))
        };
        div()
            .id("settings-panel")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .overflow_y_scroll()
            .px(px(36.))
            .py(px(30.))
            .gap(px(22.))
            // Enables selection of the SelectableText runs (command, activity).
            .child(TextSelectionLayer)
            .child(
                div()
                    .text_size(px(22.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Настройки"),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Сеть"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(20.))
                    .child(
                        div()
                            .w(px(260.))
                            .child(label("Proxy", "HTTP(S) proxy для отправки запросов")),
                    )
                    .child(
                        div()
                            .w(px(360.))
                            .child(Input::new(&self.proxy).id("settings-proxy").h(px(30.))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(20.))
                    .child(
                        div()
                            .w(px(260.))
                            .child(label("SSL verify", "Проверять сертификат TLS")),
                    )
                    .child(
                        Button::new(
                            "settings-ssl",
                            if self.settings.ssl_verify {
                                "Включено"
                            } else {
                                "Выключено"
                            },
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings.ssl_verify = !this.settings.ssl_verify;
                            cx.emit(SettingsPanelEvent::Changed(this.settings.clone()));
                            cx.notify();
                        })),
                    ),
            )
            .child({
                let view = cx.entity().downgrade();
                let selected = match self.settings.http_version {
                    HttpVersionPref::Auto => 0,
                    HttpVersionPref::Http1 => 1,
                    HttpVersionPref::Http2 => 2,
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(20.))
                    .child(div().w(px(260.)).child(label(
                        "Версия HTTP",
                        "Auto: HTTP/2 через ALPN при TLS, иначе HTTP/1.1. HTTP/2 для http:// — h2c",
                    )))
                    .child(
                        Segmented::new("settings-http-version")
                            .items(["Auto", "HTTP/1.1", "HTTP/2"])
                            .selected(selected)
                            .on_select(move |ix, _, cx| {
                                view.update(cx, |this, cx| {
                                    this.settings.http_version = match ix {
                                        1 => HttpVersionPref::Http1,
                                        2 => HttpVersionPref::Http2,
                                        _ => HttpVersionPref::Auto,
                                    };
                                    cx.emit(SettingsPanelEvent::Changed(this.settings.clone()));
                                    cx.notify();
                                })
                                .ok();
                            }),
                    )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(20.))
                    .child(div().w(px(260.)).child(label(
                        "Сохранять cookies между запросами",
                        "Отдельно для каждой коллекции и несохранённых запросов; до закрытия приложения",
                    )))
                    .child(
                        Button::new(
                            "settings-cookies",
                            if self.settings.cookies_enabled {
                                "Включено"
                            } else {
                                "Выключено"
                            },
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings.cookies_enabled = !this.settings.cookies_enabled;
                            cx.emit(SettingsPanelEvent::Changed(this.settings.clone()));
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(20.))
                    .child(div().w(px(260.)).child(label(
                        "Timeout",
                        "Время ожидания ответа в секундах (1–3600)",
                    )))
                    .child(
                        div()
                            .w(px(110.))
                            .child(Input::new(&self.timeout).id("settings-timeout").h(px(30.))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(20.))
                    .child(div().w(px(260.)).child(label(
                        "Редиректы",
                        "Автоматически переходить по ответам 3xx",
                    )))
                    .child(
                        Button::new(
                            "settings-follow-redirects",
                            if self.settings.follow_redirects {
                                "Включено"
                            } else {
                                "Выключено"
                            },
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings.follow_redirects = !this.settings.follow_redirects;
                            cx.emit(SettingsPanelEvent::Changed(this.settings.clone()));
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(20.))
                    .child(div().w(px(260.)).child(label(
                        "Максимум редиректов",
                        "Ограничение цепочки переходов (1–50)",
                    )))
                    .child(
                        div().w(px(110.)).child(
                            Input::new(&self.redirect_limit)
                                .id("settings-redirect-limit")
                                .h(px(30.)),
                        ),
                    ),
            )
            .child(self.render_mcp(cx))
            .child(
                div()
                    .pt(px(16.))
                    .border_t_1()
                    .border_color(p.border)
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("О приложении"),
            )
            .child(div().text_color(p.text_dim).child(format!(
                "Rivet {} · HTTP-клиент для API",
                env!("RIVET_VERSION")
            )))
            .child(
                div()
                    .text_color(p.text_dim)
                    .child(format!("Данные проекта: {}", self.data_path)),
            )
            .child(div().text_color(p.text_dim).child(
                "Поддерживаемые файлы: OpenAPI 3.x, Swagger 2.0, Postman Collection 2.0/2.1",
            ))
            .test_support()
    }
}
