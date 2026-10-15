//! Command palette (⌘K): a centered floating modal with a search input and a
//! fuzzy-filtered list of requests and commands. See PLAN.md (app::palette).
//!
//! The integrator owns an `Entity<CommandPalette>`, renders it as the last
//! child of the window root while open (it overlays everything through
//! `deferred`), calls [`CommandPalette::focus`] on open and closes it on
//! [`PaletteEvent::Picked`] / [`PaletteEvent::Dismissed`].

use std::ops::Range;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::model::Method;
use crate::theme::{ActivePalette, MONO};
use crate::ui::{IconName, Kbd, MethodBadge, icon};

actions!(command_palette, [SelectPrev, SelectNext, Confirm, Dismiss]);

/// Key context of the palette modal.
pub const CONTEXT: &str = "CommandPalette";

const WIDTH: f32 = 560.;
const TOP_OFFSET: f32 = 80.;
const ROW_HEIGHT: f32 = 40.;
const MAX_VISIBLE_ROWS: usize = 10;

/// Registers the palette keybindings (Up/Down, ctrl-p/ctrl-n, Enter, Escape).
///
/// Bound both on the palette and on its focused search input, so they win over
/// the input's own cursor/escape bindings.
pub fn bind_keys(cx: &mut App) {
    let mut bindings = Vec::new();
    for ctx in [CONTEXT, "CommandPalette > Input"] {
        bindings.extend([
            KeyBinding::new("up", SelectPrev, Some(ctx)),
            KeyBinding::new("ctrl-p", SelectPrev, Some(ctx)),
            KeyBinding::new("down", SelectNext, Some(ctx)),
            KeyBinding::new("ctrl-n", SelectNext, Some(ctx)),
            KeyBinding::new("enter", Confirm, Some(ctx)),
            KeyBinding::new("escape", Dismiss, Some(ctx)),
        ]);
    }
    cx.bind_keys(bindings);
}

/// One searchable entry: a saved/spec request or an app command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteItem {
    pub id: String,
    pub title: String,
    /// URL/path for requests, category for commands.
    pub subtitle: String,
    pub method: Option<Method>,
    /// Shortcut hint shown on the right, e.g. `⌘T`.
    pub kbd: Option<String>,
}

impl PaletteItem {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        subtitle: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            subtitle: subtitle.into(),
            method: None,
            kbd: None,
        }
    }

    pub fn method(mut self, method: Method) -> Self {
        self.method = Some(method);
        self
    }

    pub fn kbd(mut self, kbd: impl Into<String>) -> Self {
        self.kbd = Some(kbd.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteEvent {
    Picked(String),
    Dismissed,
}

// ---------------------------------------------------------------------------
// Fuzzy matching
// ---------------------------------------------------------------------------

const SCORE_MATCH: i64 = 16;
const BONUS_CONSECUTIVE: i64 = 14;
const BONUS_WORD_START: i64 = 8;
const BONUS_PREFIX: i64 = 15;
const PENALTY_GAP: i64 = 1;
const PENALTY_LEADING_MAX: i64 = 10;
const NONE: i64 = i64::MIN / 4;

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Case-insensitive fuzzy subsequence match of `query` against `text`.
///
/// Returns the best score and the matched *char* indices into `text`, or
/// `None` when `query` is not a subsequence. Whitespace in `query` is ignored.
/// Consecutive runs, word starts (after a separator or a camelCase hump) and a
/// match at the very beginning score higher; skipped characters cost a little.
pub fn fuzzy_match(query: &str, text: &str) -> Option<(i64, Vec<usize>)> {
    let q: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(fold)
        .collect();
    if q.is_empty() {
        return Some((0, Vec::new()));
    }
    let raw: Vec<char> = text.chars().collect();
    let t: Vec<char> = raw.iter().copied().map(fold).collect();
    let (n, m) = (q.len(), t.len());
    if n > m {
        return None;
    }
    // Cheap subsequence pre-check.
    {
        let mut qi = 0;
        for &c in &t {
            if qi < n && c == q[qi] {
                qi += 1;
            }
        }
        if qi < n {
            return None;
        }
    }

    let bonus: Vec<i64> = (0..m)
        .map(|j| {
            let word_start = if j == 0 {
                true
            } else {
                let prev = raw[j - 1];
                let cur = raw[j];
                !prev.is_alphanumeric()
                    || (prev.is_lowercase() && cur.is_uppercase())
                    || (!prev.is_numeric() && cur.is_numeric())
            };
            if word_start && raw[j].is_alphanumeric() {
                BONUS_WORD_START
            } else {
                0
            }
        })
        .collect();

    // d[i][j]: best score with q[i] matched exactly at t[j].
    // best[i][j]: best d[i][k] - gap(k..=j) for k <= j, with its argmax in pos.
    let mut d = vec![NONE; n * m];
    let mut from_consecutive = vec![false; n * m];
    let mut best = vec![NONE; n * m];
    let mut pos = vec![0usize; n * m];
    let at = |i: usize, j: usize| i * m + j;

    for i in 0..n {
        for j in 0..m {
            if t[j] == q[i] {
                let base = SCORE_MATCH + bonus[j];
                if i == 0 {
                    let lead = (j as i64 * PENALTY_GAP).min(PENALTY_LEADING_MAX);
                    d[at(i, j)] = base - lead + if j == 0 { BONUS_PREFIX } else { 0 };
                } else if j > 0 {
                    let consec = d[at(i - 1, j - 1)];
                    let gapped = best[at(i - 1, j - 1)];
                    let consec_score = if consec > NONE {
                        consec + BONUS_CONSECUTIVE
                    } else {
                        NONE
                    };
                    // `best` already includes `consec` with zero gap; prefer the
                    // consecutive path on ties.
                    if consec_score >= gapped && consec_score > NONE {
                        d[at(i, j)] = base + consec_score;
                        from_consecutive[at(i, j)] = true;
                    } else if gapped > NONE {
                        d[at(i, j)] = base + gapped;
                    }
                }
            }
            let here = d[at(i, j)];
            let carried = if j > 0 && best[at(i, j - 1)] > NONE {
                best[at(i, j - 1)] - PENALTY_GAP
            } else {
                NONE
            };
            if here > NONE && here >= carried {
                best[at(i, j)] = here;
                pos[at(i, j)] = j;
            } else {
                best[at(i, j)] = carried;
                pos[at(i, j)] = if j > 0 { pos[at(i, j - 1)] } else { 0 };
            }
        }
    }

    // Trailing characters are free: take the best end position.
    let (mut j, score) = (0..m)
        .map(|j| (j, d[at(n - 1, j)]))
        .filter(|&(_, s)| s > NONE)
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))?;

    let mut indices = vec![0; n];
    for i in (0..n).rev() {
        indices[i] = j;
        if i == 0 {
            break;
        }
        j = if from_consecutive[at(i, j)] {
            j - 1
        } else {
            pos[at(i - 1, j - 1)]
        };
    }
    Some((score, indices))
}

/// Converts char indices into byte ranges of `text`, merging adjacent ones.
fn char_indices_to_ranges(text: &str, indices: &[usize]) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut wanted = indices.iter().copied().peekable();
    for (ci, (bi, c)) in text.char_indices().enumerate() {
        let Some(&next) = wanted.peek() else { break };
        if next == ci {
            wanted.next();
            let r = bi..bi + c.len_utf8();
            match ranges.last_mut() {
                Some(last) if last.end == r.start => last.end = r.end,
                _ => ranges.push(r),
            }
        }
    }
    ranges
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Hit {
    item: usize,
    title: Vec<usize>,
    subtitle: Vec<usize>,
}

pub struct CommandPalette {
    items: Vec<PaletteItem>,
    hits: Vec<Hit>,
    selected: usize,
    query: String,
    input: Entity<InputState>,
    scroll: UniformListScrollHandle,
    _subscription: Subscription,
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

impl Focusable for CommandPalette {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl CommandPalette {
    pub fn new(items: Vec<PaletteItem>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Поиск запросов и команд…"));
        let subscription =
            cx.subscribe_in(&input, window, |this, state, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    this.query = state.read(cx).value().to_string();
                    this.refilter();
                    this.selected = 0;
                    this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                    cx.notify();
                }
            });
        let mut this = Self {
            items,
            hits: Vec::new(),
            selected: 0,
            query: String::new(),
            input,
            scroll: UniformListScrollHandle::new(),
            _subscription: subscription,
        };
        this.refilter();
        this
    }

    /// Replaces the items, keeping the current query (results are refiltered).
    pub fn set_items(&mut self, items: Vec<PaletteItem>, _: &mut Window, cx: &mut Context<Self>) {
        let selected_id = self.selected_id().map(str::to_owned);
        self.items = items;
        self.refilter();
        self.selected = selected_id
            .and_then(|id| self.hits.iter().position(|h| self.items[h.item].id == id))
            .unwrap_or(0);
        cx.notify();
    }

    pub fn items(&self) -> &[PaletteItem] {
        &self.items
    }

    /// Current search text.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Replaces the search text (as if typed).
    pub fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |s, cx| s.set_value(query.to_owned(), window, cx));
        self.query = query.to_owned();
        self.refilter();
        self.selected = 0;
        cx.notify();
    }

    /// Clears the query and selection; call before re-opening.
    pub fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_query("", window, cx);
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
    }

    /// Focuses the search input (call when the palette opens).
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |s, cx| s.focus(window, cx));
    }

    /// Ids of the filtered results, best match first.
    pub fn visible_ids(&self) -> Vec<&str> {
        self.hits
            .iter()
            .map(|h| self.items[h.item].id.as_str())
            .collect()
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected_id(&self) -> Option<&str> {
        self.hits
            .get(self.selected)
            .map(|h| self.items[h.item].id.as_str())
    }

    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        self.move_selection(1, cx);
    }

    pub fn select_prev(&mut self, cx: &mut Context<Self>) {
        self.move_selection(-1, cx);
    }

    /// Emits `Picked` for the selected result (no-op without results).
    pub fn confirm(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.selected_id() {
            cx.emit(PaletteEvent::Picked(id.to_owned()));
        }
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        cx.emit(PaletteEvent::Dismissed);
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.hits.len();
        if len == 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(len as isize) as usize;
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn refilter(&mut self) {
        let query: String = self.query.chars().filter(|c| !c.is_whitespace()).collect();
        if query.is_empty() {
            self.hits = (0..self.items.len())
                .map(|item| Hit {
                    item,
                    title: Vec::new(),
                    subtitle: Vec::new(),
                })
                .collect();
        } else {
            let mut scored: Vec<(i64, Hit)> = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(item, it)| {
                    let title = fuzzy_match(&query, &it.title);
                    let subtitle = fuzzy_match(&query, &it.subtitle).map(|(s, ix)| (s / 2, ix));
                    match (title, subtitle) {
                        (Some((ts, ti)), Some((ss, _))) if ts >= ss => Some((
                            ts,
                            Hit {
                                item,
                                title: ti,
                                subtitle: Vec::new(),
                            },
                        )),
                        (Some((ts, ti)), None) => Some((
                            ts,
                            Hit {
                                item,
                                title: ti,
                                subtitle: Vec::new(),
                            },
                        )),
                        (_, Some((ss, si))) => Some((
                            ss,
                            Hit {
                                item,
                                title: Vec::new(),
                                subtitle: si,
                            },
                        )),
                        (None, None) => None,
                    }
                })
                .collect();
            // Stable: equal scores keep the given order.
            scored.sort_by_key(|s| std::cmp::Reverse(s.0));
            self.hits = scored.into_iter().map(|(_, h)| h).collect();
        }
        if self.selected >= self.hits.len() {
            self.selected = 0;
        }
    }

    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = *cx.palette();
        let hit = &self.hits[ix];
        let item = &self.items[hit.item];
        let selected = ix == self.selected;
        let mark = HighlightStyle {
            color: Some(p.accent_hi),
            font_weight: Some(FontWeight::SEMIBOLD),
            ..Default::default()
        };
        let highlighted = |text: &str, indices: &[usize]| {
            let ranges = char_indices_to_ranges(text, indices);
            StyledText::new(SharedString::from(text.to_owned()))
                .with_highlights(ranges.into_iter().map(|r| (r, mark)))
        };

        div()
            .id(("palette-item", ix))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .h(px(ROW_HEIGHT))
            .px(px(10.))
            .rounded(px(7.))
            .cursor_pointer()
            .when(selected, |s| s.bg(p.accent.opacity(0.16)))
            .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                if this.selected != ix {
                    this.selected = ix;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.selected = ix;
                this.confirm(cx);
            }))
            .child(match item.method {
                Some(m) => MethodBadge::new(m).width(40.).into_any_element(),
                None => div()
                    .w(px(40.))
                    .flex_none()
                    .flex()
                    .child(icon(IconName::ChevronRight, 13., p.text_dim))
                    .into_any_element(),
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_baseline()
                    .gap(px(8.))
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(260.))
                            .truncate()
                            .text_size(px(13.))
                            .text_color(p.text)
                            .child(highlighted(&item.title, &hit.title)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(p.text_dim)
                            .when(item.method.is_some(), |s| s.font_family(MONO))
                            .child(highlighted(&item.subtitle, &hit.subtitle)),
                    ),
            )
            .when_some(item.kbd.clone(), |s, k| s.child(Kbd::new(k)))
            .test_support()
            .into_any_element()
    }
}

impl Render for CommandPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *cx.palette();
        let viewport = window.viewport_size();
        let count = self.hits.len();
        let visible = count.min(MAX_VISIBLE_ROWS);

        let results = if count == 0 {
            div()
                .id("palette-empty")
                .flex()
                .items_center()
                .justify_center()
                .h(px(64.))
                .text_size(px(12.5))
                .text_color(p.text_dim)
                .child("Ничего не найдено")
                .test_support()
                .into_any_element()
        } else {
            uniform_list(
                "palette-list",
                count,
                cx.processor(|this, range: Range<usize>, _, cx| {
                    range.map(|ix| this.render_row(ix, cx)).collect()
                }),
            )
            .track_scroll(&self.scroll)
            .h(px(visible as f32 * ROW_HEIGHT))
            .into_any_element()
        };

        let modal = div()
            .id("command-palette")
            .key_context(CONTEXT)
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.select_prev(cx)))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.select_next(cx)))
            .on_action(cx.listener(|this, _: &Confirm, _, cx| this.confirm(cx)))
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| this.dismiss(cx)))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .flex()
            .flex_col()
            .w(px(WIDTH))
            .max_w(viewport.width - px(32.))
            .bg(p.surface)
            .border_1()
            .border_color(p.border)
            .rounded(px(12.))
            .shadow(vec![
                BoxShadow {
                    color: black().opacity(0.45),
                    offset: point(px(0.), px(24.)),
                    blur_radius: px(64.),
                    spread_radius: px(0.),
                    inset: false,
                },
                BoxShadow {
                    color: black().opacity(0.25),
                    offset: point(px(0.), px(4.)),
                    blur_radius: px(12.),
                    spread_radius: px(0.),
                    inset: false,
                },
            ])
            .overflow_hidden()
            .text_color(p.text)
            .child(
                div()
                    .flex()
                    .items_center()
                    .px(px(12.))
                    .py(px(8.))
                    .border_b_1()
                    .border_color(p.border)
                    .child(
                        Input::new(&self.input)
                            .id("palette-input")
                            .prefix(icon(IconName::Search, 15., p.text_dim))
                            .appearance(false)
                            .text_size(px(14.))
                            .h(px(36.)),
                    ),
            )
            .child(div().flex().flex_col().p(px(6.)).child(results))
            .test_support();

        let backdrop = div()
            .id("palette-backdrop")
            .w(viewport.width)
            .h(viewport.height)
            .flex()
            .flex_col()
            .items_center()
            .pt(px(TOP_OFFSET))
            .bg(black().opacity(0.45))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.dismiss(cx)),
            )
            .child(modal)
            .test_support();

        deferred(anchored().position(point(px(0.), px(0.))).child(backdrop)).with_priority(10)
    }
}

#[cfg(test)]
mod tests {
    use super::{char_indices_to_ranges, fuzzy_match};

    #[test]
    fn ranges_merge_and_handle_multibyte() {
        assert_eq!(char_indices_to_ranges("abc", &[0, 1]), vec![0..2]);
        // "Заказ": each Cyrillic char is 2 bytes.
        assert_eq!(char_indices_to_ranges("Заказ", &[0, 2]), vec![0..2, 4..6]);
    }

    #[test]
    fn fuzzy_basics() {
        assert_eq!(fuzzy_match("", "anything"), Some((0, vec![])));
        assert!(fuzzy_match("xyz", "orders").is_none());
        let (_, ix) = fuzzy_match("ord", "Create order").unwrap();
        assert_eq!(ix, vec![7, 8, 9]);
    }
}
