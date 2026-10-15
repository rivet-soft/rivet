//! Application settings, separate from request and environment editors.

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::*;

use crate::core::{HttpVersionPref, Settings};
use crate::theme::ActivePalette;
use crate::ui::{Button, Segmented};

pub struct SettingsPanel {
    settings: Settings,
    proxy: Entity<InputState>,
    timeout: Entity<InputState>,
    redirect_limit: Entity<InputState>,
    data_path: String,
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
            _subscriptions: subscriptions,
        }
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
