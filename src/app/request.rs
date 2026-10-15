use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::sidebar::{COLLECTIONS_SIDEBAR_WIDTH, HISTORY_SIDEBAR_WIDTH};
use super::ws_view::{WsBadge, WsStatus};
use super::{MODE_RAIL_WIDTH, Workspace, is_destructive};
use crate::core::{Method, new_id, vars, ws};
use crate::theme::{ActivePalette, MONO};
use crate::ui::{
    Button, ButtonVariant, ConfirmPopover, IconButton, IconName, MethodBadge, StatusDot, PANE_SPLITTER_WIDTH,
    PaneSplitter, Pill, Placement, Popover, Tab, TabAction, TabActionsMenu, TabBar, icon,
    shortcut_hint, var_token_element,
};

const MIN_REQUEST_PANE_WIDTH: f32 = 280.;
const MIN_RESPONSE_PANE_WIDTH: f32 = 280.;

fn request_pane_width(available: f32, ratio: f32) -> f32 {
    let usable = (available - PANE_SPLITTER_WIDTH).max(0.);
    let request_min = MIN_REQUEST_PANE_WIDTH.min(usable / 2.);
    let response_min = MIN_RESPONSE_PANE_WIDTH.min(usable / 2.);
    (usable * ratio).clamp(request_min, usable - response_min)
}

impl Workspace {
    fn request_body_width(&self, window: &Window) -> f32 {
        let sidebar_width = if self.sidebar_visible {
            if self.rail == crate::model::RailMode::History {
                HISTORY_SIDEBAR_WIDTH
            } else {
                COLLECTIONS_SIDEBAR_WIDTH
            }
        } else {
            0.
        };
        (window.viewport_size().width.as_f32() - MODE_RAIL_WIDTH - sidebar_width).max(0.)
    }

    pub(super) fn move_request_split(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.request_split_dragging {
            return;
        }
        if event.pressed_button.is_none() {
            self.stop_request_split(cx);
            return;
        }
        let Some(bounds) = self.request_split_bounds.get() else {
            return;
        };
        let available = bounds.size.width.as_f32();
        let usable = available - PANE_SPLITTER_WIDTH;
        if usable <= 0. {
            return;
        }
        let cursor_x = event.position.x.as_f32() - bounds.origin.x.as_f32();
        let left = request_pane_width(available, (cursor_x - PANE_SPLITTER_WIDTH / 2.) / usable);
        let ratio = left / usable;
        if (self.request_split_ratio - ratio).abs() > f32::EPSILON {
            self.request_split_ratio = ratio;
            cx.notify();
        }
    }

    pub(super) fn stop_request_split(&mut self, cx: &mut Context<Self>) {
        if self.request_split_dragging {
            self.request_split_dragging = false;
            cx.notify();
        }
    }

    pub(super) fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let ws_view = self.ws_view.read(cx);
        let bar = TabBar::new("tabbar")
            .tabs(self.tabs.iter().enumerate().map(|(ix, t)| {
                let select_view = view.clone();
                let dismiss_view = view.clone();
                let name = if t.request.name.is_empty() {
                    "Untitled".to_string()
                } else {
                    t.request.name.clone()
                };
                let is_ws = t.request.is_ws();
                let label = if is_ws {
                    format!("{} · {name}", ws::scheme_label(&t.request.url))
                } else {
                    name
                };
                let tab = Tab::new(("tab", ix), label);
                let tab = if is_ws { tab } else { tab.method(t.request.method) };
                tab.active(self.active_tab == Some(ix))
                    .dirty(t.is_sending() || (is_ws && ws_view.is_live(&t.request.id)))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.activate_tab(ix, window, cx)),
                    )
                    .on_close(
                        cx.listener(move |this, _, window, cx| this.close_tab_at(ix, window, cx)),
                    )
                    .menu(
                        self.tab_menu == Some(ix),
                        TabActionsMenu::new(("tab-actions-menu", ix)).on_select(
                            move |action, window, cx| {
                                select_view
                                    .update(cx, |this, cx| {
                                        this.run_tab_action(ix, action, window, cx)
                                    })
                                    .ok();
                            },
                        ),
                    )
                    .on_menu_click(cx.listener(move |this, _, _, cx| {
                        this.tab_menu = (this.tab_menu != Some(ix)).then_some(ix);
                        cx.notify();
                    }))
                    .on_context_menu(cx.listener(move |this, _, _, cx| {
                        this.tab_menu = Some(ix);
                        cx.notify();
                    }))
                    .on_menu_dismiss(move |_, cx| {
                        dismiss_view
                            .update(cx, |this, cx| {
                                this.tab_menu = None;
                                cx.notify();
                            })
                            .ok();
                    })
            }));
        let p = *cx.palette();
        let item = |id: &'static str, label: &'static str, badge: AnyElement| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(28.))
                .px(px(8.))
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(p.surface_hi))
                .child(div().w(px(34.)).child(badge))
                .child(label)
                .test_support()
        };
        let menu = div()
            .flex()
            .flex_col()
            .p(px(4.))
            .child(
                item(
                    "new-tab-http",
                    "HTTP запрос",
                    MethodBadge::new(Method::Get).into_any_element(),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.new_tab_menu_open = false;
                    this.new_tab(window, cx)
                })),
            )
            .child(
                item("new-tab-ws", "WebSocket", WsBadge::new(false).into_any_element())
                    .on_click(cx.listener(|this, _, window, cx| this.new_ws_tab(window, cx))),
            );
        let new_menu = Popover::new("new-tab-popover")
            .open(self.new_tab_menu_open)
            .width(190.)
            .placement(Placement::Below)
            .trigger(IconButton::new("new-tab-menu", IconName::Plus).on_click(cx.listener(
                |this, _, _, cx| {
                    this.new_tab_menu_open = !this.new_tab_menu_open;
                    cx.notify();
                },
            )))
            .child(menu)
            .on_dismiss(move |_, cx| {
                view.update(cx, |this, cx| {
                    this.new_tab_menu_open = false;
                    cx.notify();
                })
                .ok();
            });
        div()
            .flex()
            .flex_none()
            .items_start()
            .bg(p.surface)
            .child(div().flex_1().min_w_0().overflow_hidden().child(bar))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(px(36.))
                    .px(px(6.))
                    .border_b_1()
                    .border_color(p.border)
                    .child(new_menu),
            )
    }

    fn run_tab_action(
        &mut self,
        ix: usize,
        action: TabAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_menu = None;
        match action {
            TabAction::NewRequest => self.new_tab(window, cx),
            TabAction::Duplicate => {
                if let Some(tab) = self.tabs.get(ix) {
                    let mut request = tab.request.clone();
                    request.id = new_id();
                    self.tabs.insert(ix + 1, super::RequestTab::new(request));
                    self.set_active(Some(ix + 1), window, cx);
                }
            }
            TabAction::Close => self.close_tab_at(ix, window, cx),
            TabAction::ForceClose => self.force_close_tab_at(ix, window, cx),
            TabAction::CloseOthers => {
                let Some(id) = self.tabs.get(ix).map(|tab| tab.request.id.clone()) else {
                    return;
                };
                let skipped =
                    self.close_matching_tabs(|tab| tab.request.id != id, false, window, cx);
                if let Some(selected) = self.tabs.iter().position(|tab| tab.request.id == id) {
                    self.set_active(Some(selected), window, cx);
                }
                if skipped {
                    self.notify_user("Выполняющиеся запросы остались открыты", cx);
                }
            }
            TabAction::CloseAll | TabAction::ForceCloseAll => {
                let force = action == TabAction::ForceCloseAll;
                let skipped = self.close_matching_tabs(|_| true, force, window, cx);
                if skipped {
                    self.notify_user("Выполняющиеся запросы остались открыты", cx);
                }
            }
        }
        cx.notify();
    }

    fn close_matching_tabs(
        &mut self,
        matches: impl Fn(&super::RequestTab) -> bool,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut skipped = false;
        for ix in (0..self.tabs.len()).rev() {
            if !matches(&self.tabs[ix]) {
                continue;
            }
            if !force && self.tabs[ix].is_sending() {
                skipped = true;
                continue;
            }
            self.force_close_tab_at(ix, window, cx);
        }
        skipped
    }

    pub(super) fn render_request_line(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *cx.palette();
        let Some(request) = self.active() else {
            return div().id("request-line").into_any_element();
        };
        if request.is_ws() {
            return self.render_ws_request_line(cx).into_any_element();
        }
        let production = self.env().is_some_and(|e| e.production);
        let curl_copied = self.notice.as_deref() == Some("cURL скопирован");
        let view = cx.entity().downgrade();
        let current = request.method;
        let sending = self.active_tab.is_some_and(|ix| self.tabs[ix].is_sending());

        let trigger = div()
            .id("method-select")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(32.))
            .px(px(12.))
            .border_r_1()
            .border_color(p.border)
            .cursor_pointer()
            .hover(|s| s.bg(p.surface_hi))
            .on_click(cx.listener(|this, _, _, cx| {
                this.method_menu_open = !this.method_menu_open;
                cx.notify();
            }))
            .child(MethodBadge::new(current).full().width(52.))
            .child(icon(IconName::ChevronDown, 12., p.text_dim))
            .test_support();

        let menu = div()
            .flex()
            .flex_col()
            .p(px(4.))
            .children(Method::ALL.map(|m| {
                let view = view.clone();
                div()
                    .id(("method", m as usize))
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(28.))
                    .px(px(8.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(p.surface_hi))
                    .on_click(move |_, window, cx| {
                        view.update(cx, |this, cx| this.set_method(m, window, cx))
                            .ok();
                    })
                    .child(MethodBadge::new(m).full())
                    .when(m == current, |d| {
                        d.child(icon(IconName::Check, 13., p.accent))
                    })
                    .test_support()
            }));

        let method = Popover::new("method-menu")
            .open(self.method_menu_open)
            .width(140.)
            .trigger(trigger)
            .child(menu)
            .on_dismiss({
                let view = view.clone();
                move |_, cx| {
                    view.update(cx, |this, cx| {
                        this.method_menu_open = false;
                        cx.notify();
                    })
                    .ok();
                }
            });

        let url = Input::new(&self.url_input)
            .id("url")
            .token(|ctx, _, cx| var_token_element(ctx, cx))
            .appearance(false)
            .font_family(MONO)
            .text_size(px(12.))
            .h(px(32.));

        let send_button = if sending {
            Button::new("send", "Отменить")
                .icon(IconName::X)
                .kbd(shortcut_hint("⌘.", "Ctrl+."))
                .on_click(cx.listener(|this, _, _, cx| this.cancel(cx)))
        } else {
            Button::new("send", "Send")
                .variant(if production {
                    ButtonVariant::Danger
                } else {
                    ButtonVariant::Primary
                })
                .icon(IconName::Play)
                .kbd(shortcut_hint("⌘↵", "Ctrl+Enter"))
                .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
        };

        let resolved = vars::resolve(request, self.env());
        let resolved_url = resolved.as_ref().map(|r| r.url.clone()).unwrap_or_default();
        let missing = resolved.map(|r| r.missing_vars).unwrap_or_default();

        let send = ConfirmPopover::new("prod-confirm", resolved_url)
            .open(self.confirm_open && production && is_destructive(current))
            .placement(Placement::BelowEnd)
            .title(format!(
                "Вы отправляете {} в Production. Продолжить?",
                current.full_label()
            ))
            .confirm_label("Отправить в Prod")
            .trigger(send_button)
            .on_cancel({
                let view = view.clone();
                move |_, cx| {
                    view.update(cx, |this, cx| {
                        this.confirm_open = false;
                        cx.notify();
                    })
                    .ok();
                }
            })
            .on_confirm(move |window, cx| {
                view.update(cx, |this, cx| this.dispatch(window, cx)).ok();
            });

        div()
            .id("request-line")
            .flex()
            .flex_none()
            .flex_col()
            .gap(px(6.))
            .px(px(14.))
            .py(px(10.))
            .border_b_1()
            .border_color(p.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .h(px(34.))
                            .rounded(px(8.))
                            .bg(p.surface)
                            .border_1()
                            // Prod gets a danger outline so the target is never ambiguous.
                            .border_color(if production {
                                p.danger.opacity(0.6)
                            } else {
                                p.border
                            })
                            .child(method)
                            .child(div().flex_1().min_w_0().px(px(4.)).child(url)),
                    )
                    .child(send)
                    .child(
                        Button::new("save", "Save")
                            .kbd(shortcut_hint("⌘S", "Ctrl+S"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.save_request(window, cx)),
                            ),
                    )
                    .child(
                        Button::new(
                            "copy-curl",
                            if curl_copied {
                                "Скопировано"
                            } else {
                                "Copy cURL"
                            },
                        )
                        .icon(if curl_copied {
                            IconName::Check
                        } else {
                            IconName::Copy
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.copy_curl(cx))),
                    ),
            )
            .when(!missing.is_empty(), |d| {
                d.child(
                    div()
                        .id("missing-vars")
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(11.))
                        .text_color(p.warn)
                        .child(icon(IconName::TriangleAlert, 12., p.warn))
                        .child("Не определены в окружении:")
                        .children(
                            missing
                                .into_iter()
                                .map(|m| Pill::new(format!("{{{{{m}}}}}")).color(p.warn).mono()),
                        )
                        .test_support(),
                )
            })
            .into_any_element()
    }

    fn render_ws_request_line(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = *cx.palette();
        let Some(request) = self.active() else {
            return div().id("request-line");
        };
        let status = self.ws_view.read(cx).status_of(&request.id);
        let secure = ws::scheme_label(&request.url) == "WSS";
        let scheme = div()
            .id("ws-scheme")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(32.))
            .px(px(12.))
            .border_r_1()
            .border_color(p.border)
            .child(WsBadge::new(secure).width(30.))
            .child(StatusDot::new(match status {
                WsStatus::Open => p.online,
                WsStatus::Connecting => p.amber,
                WsStatus::Failed => p.danger,
                _ => p.text_dim,
            }))
            .test_support();
        let url = Input::new(&self.url_input)
            .id("url")
            .token(|ctx, _, cx| var_token_element(ctx, cx))
            .appearance(false)
            .font_family(MONO)
            .text_size(px(12.))
            .h(px(32.));
        let connect = match status {
            WsStatus::Open => Button::new("ws-connect", "Disconnect")
                .danger()
                .icon(IconName::Unplug),
            WsStatus::Connecting => Button::new("ws-connect", "Подключение…")
                .icon(IconName::LoaderCircle),
            _ => Button::new("ws-connect", "Connect")
                .primary()
                .icon(IconName::Play),
        }
        .on_click(cx.listener(|this, _, _, cx| this.ws_toggle(cx)));
        let missing = ws::resolve(request, self.env())
            .map(|r| r.missing_vars)
            .unwrap_or_default();
        div()
            .id("request-line")
            .flex()
            .flex_none()
            .flex_col()
            .gap(px(6.))
            .px(px(14.))
            .py(px(10.))
            .border_b_1()
            .border_color(p.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .h(px(34.))
                            .rounded(px(8.))
                            .bg(p.surface)
                            .border_1()
                            .border_color(match status {
                                WsStatus::Open => p.online.opacity(0.6),
                                WsStatus::Connecting => p.amber.opacity(0.6),
                                _ => p.border,
                            })
                            .child(scheme)
                            .child(div().flex_1().min_w_0().px(px(4.)).child(url)),
                    )
                    .child(connect)
                    .child(
                        Button::new("save", "Save")
                            .kbd(shortcut_hint("⌘S", "Ctrl+S"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.save_request(window, cx)),
                            ),
                    ),
            )
            .when(!missing.is_empty(), |d| {
                d.child(
                    div()
                        .id("missing-vars")
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(11.))
                        .text_color(p.warn)
                        .child(icon(IconName::TriangleAlert, 12., p.warn))
                        .child("Не определены в окружении:")
                        .children(
                            missing
                                .into_iter()
                                .map(|m| Pill::new(format!("{{{{{m}}}}}")).color(p.warn).mono()),
                        )
                        .test_support(),
                )
            })
    }

    pub(super) fn render_workspace_body(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = *cx.palette();
        if self.active().is_none() {
            return div()
                .flex()
                .flex_1()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .text_color(p.text_dim)
                .child(
                    div()
                        .text_size(px(15.))
                        .text_color(p.text)
                        .child("Нет открытых запросов"),
                )
                .child("Выберите запрос слева или создайте новый")
                .child(
                    Button::new("empty-new-tab", "Новый запрос")
                        .primary()
                        .kbd(shortcut_hint("⌘T", "Ctrl+T"))
                        .on_click(cx.listener(|this, _, window, cx| this.new_tab(window, cx))),
                )
                .into_any_element();
        }
        if self.active().is_some_and(|r| r.is_ws()) {
            return div()
                .flex()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .child(self.ws_view.clone())
                .into_any_element();
        }
        let width = self.request_body_width(window);
        let left_width = request_pane_width(width, self.request_split_ratio);
        let bounds = self.request_split_bounds.clone();
        div()
            .id("request-response-split")
            .relative()
            .flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
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
                    .id("request-editor-pane")
                    .flex()
                    .flex_col()
                    .flex_none()
                    .w(px(left_width))
                    .min_w(px(MIN_REQUEST_PANE_WIDTH))
                    .child(self.editor.clone())
                    .test_support(),
            )
            .child(
                PaneSplitter::new("request-response-divider")
                    .active(self.request_split_dragging)
                    .on_drag_start(cx.listener(|this, _, _, cx| {
                        this.request_split_dragging = true;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("response-pane")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(MIN_RESPONSE_PANE_WIDTH))
                    .child(self.response.clone())
                    .test_support(),
            )
            .into_any_element()
    }
}
