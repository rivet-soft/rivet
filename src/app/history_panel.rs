//! Request history (rail «History», PLAN.md app::history_panel): filterable,
//! newest-first list grouped by day, virtualized with `uniform_list`.
//!
//! Times are shown in **UTC** (no timezone database without chrono).
//!
//! Element ids: `"history-filter"`, `"history-clear"`, `"history-list"`,
//! `("history-row", entry_ix)` where `entry_ix` indexes [`HistoryPanel::entries`].

use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::core::types::HistoryEntry;
use crate::theme::{ActivePalette, MONO, Palette};
use crate::ui::{Button, EmptyState, IconName, MethodBadge, Pill, icon};

const ROW_H: f32 = 30.;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryPanelEvent {
    Open(HistoryEntry),
    Clear,
}

/// One line of the virtualized list.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Item {
    Day(SharedString),
    Entry(usize),
}

pub struct HistoryPanel {
    entries: Vec<HistoryEntry>,
    filter_input: Entity<InputState>,
    filter: String,
    items: Vec<Item>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<HistoryPanelEvent> for HistoryPanel {}

impl HistoryPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Фильтр: URL, метод, статус"));
        let subscriptions = vec![cx.subscribe_in(
            &filter_input,
            window,
            |this, state, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    this.filter = state.read(cx).value().trim().to_lowercase();
                    this.rebuild_items();
                    cx.notify();
                }
            },
        )];
        Self {
            entries: Vec::new(),
            filter_input,
            filter: String::new(),
            items: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    /// Replaces the list; entries are sorted newest first.
    pub fn set_entries(&mut self, mut entries: Vec<HistoryEntry>, cx: &mut Context<Self>) {
        entries.sort_by(|a, b| b.at.cmp(&a.at));
        self.entries = entries;
        self.rebuild_items();
        cx.notify();
    }

    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// Entries matching the current filter, newest first.
    pub fn visible_entries(&self) -> Vec<&HistoryEntry> {
        self.items
            .iter()
            .filter_map(|i| match i {
                Item::Entry(ix) => Some(&self.entries[*ix]),
                Item::Day(_) => None,
            })
            .collect()
    }

    fn matches(&self, e: &HistoryEntry) -> bool {
        if self.filter.is_empty() {
            return true;
        }
        let r = &e.request;
        let status = e
            .status
            .map(|s| s.to_string())
            .unwrap_or_else(|| "err".into());
        [
            r.name.to_lowercase(),
            r.url.to_lowercase(),
            r.method.full_label().to_lowercase(),
            status,
            e.env.to_lowercase(),
        ]
        .iter()
        .any(|h| h.contains(&self.filter))
    }

    fn rebuild_items(&mut self) {
        let now = now_secs();
        let mut items = Vec::new();
        let mut last_day = None;
        for (ix, e) in self.entries.iter().enumerate() {
            if !self.matches(e) {
                continue;
            }
            let day = e.at / 86_400;
            if last_day != Some(day) {
                last_day = Some(day);
                items.push(Item::Day(day_label(e.at, now).into()));
            }
            items.push(Item::Entry(ix));
        }
        self.items = items;
    }

    fn render_item(&self, item: &Item, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        match item {
            Item::Day(label) => div()
                .flex()
                .items_end()
                .h(px(ROW_H))
                .px(px(14.))
                .pb(px(5.))
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.text_dim)
                .child(label.clone())
                .into_any_element(),
            Item::Entry(ix) => {
                let ix = *ix;
                let e = &self.entries[ix];
                let r = &e.request;
                let has_name = !r.name.is_empty() && r.name != "Untitled";
                let (status_label, status_color) = status_style(e.status, p);
                let ellipsis = || {
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                };
                div()
                    .id(("history-row", ix))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(ROW_H))
                    .mx(px(4.))
                    .px(px(10.))
                    .rounded(px(5.))
                    .cursor_pointer()
                    .hover(|s| s.bg(p.surface_hi))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(e) = this.entries.get(ix) {
                            cx.emit(HistoryPanelEvent::Open(e.clone()));
                        }
                    }))
                    .child(MethodBadge::new(r.method).width(38.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .flex_1()
                            .min_w_0()
                            .when(has_name, |s| {
                                s.child(
                                    ellipsis()
                                        .flex_none()
                                        .max_w(px(220.))
                                        .text_size(px(12.5))
                                        .text_color(p.text)
                                        .child(r.name.clone()),
                                )
                            })
                            .child(
                                ellipsis()
                                    .flex_1()
                                    .font_family(MONO)
                                    .text_size(px(11.5))
                                    .text_color(if has_name { p.text_dim } else { p.text })
                                    .child(r.url.clone()),
                            ),
                    )
                    .child(Pill::new(status_label).color(status_color).mono())
                    .child(
                        div()
                            .w(px(56.))
                            .flex_none()
                            .text_right()
                            .font_family(MONO)
                            .text_size(px(11.))
                            .text_color(p.text_dim)
                            .child(e.duration_ms.map(format_ms).unwrap_or_else(|| "—".into())),
                    )
                    .child(
                        div()
                            .w(px(38.))
                            .flex_none()
                            .text_right()
                            .font_family(MONO)
                            .text_size(px(11.))
                            .text_color(p.text_dim)
                            .child(format_hm(e.at)),
                    )
                    .test_support()
                    .into_any_element()
            }
        }
    }
}

impl Render for HistoryPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *cx.palette();
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .py(px(8.))
            .border_b_1()
            .border_color(p.border)
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.filter_input)
                        .id("history-filter")
                        .prefix(icon(IconName::Search, 13., p.text_dim))
                        .cleanable(true)
                        .h(px(28.)),
                ),
            )
            .child(
                Button::new("history-clear", "Очистить")
                    .ghost()
                    .disabled(self.entries.is_empty())
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(HistoryPanelEvent::Clear))),
            );

        let body = if self.entries.is_empty() {
            div()
                .flex_1()
                .child(
                    EmptyState::new("history-empty", "История пуста")
                        .icon(IconName::Inbox)
                        .hint("Отправленные запросы появятся здесь"),
                )
                .into_any_element()
        } else if self.items.is_empty() {
            div()
                .flex_1()
                .child(
                    EmptyState::new("history-empty", "Ничего не найдено")
                        .icon(IconName::Search)
                        .hint("Измените фильтр"),
                )
                .into_any_element()
        } else {
            uniform_list(
                "history-list",
                self.items.len(),
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .filter_map(|i| this.items.get(i).cloned())
                        .map(|item| this.render_item(&item, &p, cx))
                        .collect::<Vec<_>>()
                }),
            )
            .flex_1()
            .py(px(2.))
            .into_any_element()
        };

        div()
            .id("history-panel")
            .flex()
            .flex_col()
            .size_full()
            .bg(p.bg)
            .child(header)
            .child(body)
    }
}

/// Pill label and colour: 2xx online, 3xx accent, 4xx warn, 5xx danger, none → `ERR`.
pub fn status_style(status: Option<u16>, p: &Palette) -> (String, Hsla) {
    match status {
        None => ("ERR".into(), p.danger),
        Some(s) => {
            let c = match s {
                200..=299 => p.online,
                300..=399 => p.accent,
                400..=499 => p.warn,
                500..=599 => p.danger,
                _ => p.text_dim,
            };
            (s.to_string(), c)
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `148 ms`, `1.4 s`.
pub fn format_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else {
        format!("{:.1} s", ms as f64 / 1000.)
    }
}

/// `HH:MM` (UTC).
pub fn format_hm(at: u64) -> String {
    let s = at % 86_400;
    format!("{:02}:{:02}", s / 3600, s % 3600 / 60)
}

/// "Сегодня", "Вчера" or `DD.MM.YYYY` (UTC days) for `at` relative to `now`.
pub fn day_label(at: u64, now: u64) -> String {
    let (day, today) = (at / 86_400, now / 86_400);
    if day == today {
        "Сегодня".into()
    } else if day + 1 == today {
        "Вчера".into()
    } else {
        let (y, m, d) = civil_from_days(day as i64);
        format!("{d:02}.{m:02}.{y}")
    }
}

/// Days since 1970-01-01 → (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}
