//! Component gallery: every `ui` component in its states, wired to real view
//! state so interactions can be checked by hand and in tests.

use gpui_kit::component::TitleBar;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::model::{EnvKind, Method, RequestTab, demo_groups};
use crate::theme::{ActivePalette, Palette};
use crate::ui::*;

pub struct Gallery {
    pub kv_rows: Vec<KvRow>,
    pub clicks: usize,
    pub segment: usize,
    pub tabs: Vec<RequestTab>,
    pub active_tab: Option<usize>,
    pub folder_open: bool,
    pub selected_row: Option<usize>,
}

impl Gallery {
    pub fn new() -> Self {
        let groups = demo_groups();
        Self {
            kv_rows: vec![
                KvRow::new("customerId", "{{customerId}}").description("ID покупателя"),
                KvRow::new("limit", "20").description("Размер страницы"),
                KvRow::new("Authorization", "Bearer {{token}}")
                    .description("Отключено")
                    .enabled(false),
            ],
            clicks: 0,
            segment: 0,
            tabs: groups[0]
                .endpoints
                .iter()
                .take(3)
                .map(RequestTab::from)
                .collect(),
            active_tab: Some(0),
            folder_open: true,
            selected_row: Some(1),
        }
    }
}

impl Default for Gallery {
    fn default() -> Self {
        Self::new()
    }
}

fn section(title: &'static str, p: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .p(px(16.))
        .rounded(px(10.))
        .bg(p.surface)
        .border_1()
        .border_color(p.border)
        .child(div().text_size(px(11.)).text_color(p.text_dim).child(title))
}

fn row() -> Div {
    div().flex().flex_wrap().items_center().gap(px(10.))
}

impl Render for Gallery {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *cx.palette();
        let view = cx.entity().downgrade();

        let buttons = section("Button / IconButton", &p)
            .child(
                row()
                    .child(
                        Button::new("btn-send", "Send")
                            .primary()
                            .icon(IconName::Play)
                            .kbd(shortcut_hint("⌘↵", "Ctrl+Enter"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.clicks += 1;
                                cx.notify();
                            })),
                    )
                    .child(Button::new("btn-save", "Save").kbd(shortcut_hint("⌘S", "Ctrl+S")))
                    .child(Button::new("btn-beautify", "Beautify").ghost())
                    .child(Button::new("btn-prod", "Отправить в Prod").danger())
                    .child(Button::new("btn-disabled", "Disabled").disabled(true))
                    .child(IconButton::new("ib-more", IconName::Ellipsis))
                    .child(IconButton::new("ib-bookmark", IconName::Bookmark)),
            )
            .child(
                div()
                    .id("clicks")
                    .text_color(p.text_dim)
                    .child(format!("Send clicked: {}", self.clicks)),
            );

        let badges = section("MethodBadge / Pill / StatusDot / Kbd", &p)
            .child(
                row().children(
                    [Method::Get, Method::Post, Method::Patch, Method::Delete]
                        .map(|m| MethodBadge::new(m).full()),
                ),
            )
            .child(
                row()
                    .child(Pill::new("201 Created").color(p.online).mono())
                    .child(Pill::new("148 ms").mono())
                    .child(Pill::new("Схема ✓").color(p.online))
                    .child(Pill::new("2 расхождения").color(p.warn))
                    .child(Pill::new("ECONNREFUSED").color(p.danger).mono())
                    .child(Pill::new("3"))
                    .child(StatusDot::new(EnvKind::Dev.color(&p)))
                    .child(StatusDot::new(EnvKind::Prod.color(&p)))
                    .child(Kbd::new(shortcut_hint("⌘K", "Ctrl+K")))
                    .child(Kbd::new("Esc")),
            );

        let segmented = section("Segmented", &p).child(
            Segmented::new("seg-response")
                .items(["Pretty", "Raw", "Preview"])
                .selected(self.segment)
                .on_select({
                    let view = view.clone();
                    move |ix, _, cx| {
                        view.update(cx, |this, cx| {
                            this.segment = ix;
                            cx.notify();
                        })
                        .ok();
                    }
                }),
        );

        let tabs = section("TabBar / Tab", &p).child(
            div()
                .rounded(px(6.))
                .overflow_hidden()
                .border_1()
                .border_color(p.border)
                .child(
                    TabBar::new("tabbar")
                        .tabs(self.tabs.iter().enumerate().map(|(ix, t)| {
                            Tab::new(("tab", ix), t.label.clone())
                                .method(t.method)
                                .active(self.active_tab == Some(ix))
                                .dirty(ix == 1)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.active_tab = Some(ix);
                                    cx.notify();
                                }))
                                .on_close(cx.listener(move |this, _, _, cx| {
                                    this.close_tab(ix);
                                    cx.notify();
                                }))
                        }))
                        .on_new(cx.listener(|this, _, _, cx| {
                            this.tabs.push(RequestTab::untitled());
                            this.active_tab = Some(this.tabs.len() - 1);
                            cx.notify();
                        })),
                ),
        );

        let groups = demo_groups();
        let orders = &groups[0];
        let tree = section("SectionLabel / TreeRow", &p).child(
            div()
                .w(px(260.))
                .py(px(6.))
                .rounded(px(6.))
                .border_1()
                .border_color(p.border)
                .child(SectionLabel::new("COLLECTIONS").caps())
                .child(SectionLabel::new("Из спецификации"))
                .child(
                    TreeRow::folder("folder-orders", orders.name.clone(), self.folder_open)
                        .count(orders.endpoints.len())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.folder_open = !this.folder_open;
                            cx.notify();
                        })),
                )
                .when(self.folder_open, |d| {
                    d.children(orders.endpoints.iter().enumerate().map(|(ix, e)| {
                        TreeRow::request(("tree-row", ix), e.name.clone(), e.method)
                            .selected(self.selected_row == Some(ix))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected_row = Some(ix);
                                cx.notify();
                            }))
                    }))
                }),
        );

        let fields = section("UrlField / KeyValueTable", &p)
            .child(
                div()
                    .h(px(34.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(p.border)
                    .bg(p.surface)
                    .flex()
                    .child(UrlField::new(
                        "url-field",
                        "{{baseUrl}}/orders/{{orderId}}?expand=items",
                    )),
            )
            .child(
                KeyValueTable::new("kv-table", self.kv_rows.clone())
                    .on_toggle({
                        let view = view.clone();
                        move |ix, _, cx| {
                            view.update(cx, |this, cx| {
                                if let Some(r) = this.kv_rows.get_mut(ix) {
                                    r.enabled = !r.enabled;
                                }
                                cx.notify();
                            })
                            .ok();
                        }
                    })
                    .on_add({
                        let view = view.clone();
                        move |_, cx| {
                            view.update(cx, |this, cx| {
                                this.kv_rows.push(KvRow::new("key", "{{value}}"));
                                cx.notify();
                            })
                            .ok();
                        }
                    }),
            );

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.bg)
            .text_color(p.text)
            .text_size(px(12.5))
            .child(
                TitleBar::new().bg(p.surface).border_color(p.border).child(
                    div()
                        .flex()
                        .flex_1()
                        .items_center()
                        .justify_between()
                        .pr(px(12.))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Rivet · UI kit"),
                        )
                        .child(
                            Button::new("toggle-theme", "Тема")
                                .ghost()
                                .kbd(shortcut_hint("⌘⇧L", "Ctrl+Shift+L"))
                                .on_click(|_, _, cx| Palette::toggle(cx)),
                        ),
                ),
            )
            .child(
                div()
                    .id("gallery-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .p(px(20.))
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .child(buttons)
                    .child(badges)
                    .child(segmented)
                    .child(tabs)
                    .child(tree)
                    .child(fields),
            )
    }
}

impl Gallery {
    pub fn close_tab(&mut self, ix: usize) {
        if ix >= self.tabs.len() {
            return;
        }
        self.tabs.remove(ix);
        self.active_tab = match self.active_tab {
            _ if self.tabs.is_empty() => None,
            Some(a) if a > ix || a == self.tabs.len() => Some(a - 1),
            a => a,
        };
    }
}
