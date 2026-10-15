use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{IconName, icon, split_vars};
use crate::theme::{ActivePalette, MONO, Palette};

type IndexHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;
type AddHandler = Rc<dyn Fn(&mut Window, &mut App)>;

const KEY_W: f32 = 180.;
const DESC_W: f32 = 200.;
const TOGGLE_COL_W: f32 = 28.;

/// One line of the Params / Headers table.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KvRow {
    pub enabled: bool,
    pub key: SharedString,
    pub value: SharedString,
    pub description: SharedString,
}

impl KvRow {
    pub fn new(key: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        Self {
            enabled: true,
            key: key.into(),
            value: value.into(),
            description: SharedString::default(),
        }
    }

    pub fn description(mut self, d: impl Into<SharedString>) -> Self {
        self.description = d.into();
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// Read-only `Key | Value | Description` grid with enable toggles and a trailing
/// "+ Add" row. Element ids inside the table: `("toggle", ix)`, `("row", ix)`, `"add"`.
#[derive(IntoElement)]
pub struct KeyValueTable {
    id: ElementId,
    rows: Vec<KvRow>,
    on_toggle: Option<IndexHandler>,
    on_add: Option<AddHandler>,
}

impl KeyValueTable {
    pub fn new(id: impl Into<ElementId>, rows: Vec<KvRow>) -> Self {
        Self {
            id: id.into(),
            rows,
            on_toggle: None,
            on_add: None,
        }
    }

    pub fn on_toggle(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(f));
        self
    }

    pub fn on_add(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_add = Some(Rc::new(f));
        self
    }
}

fn cell() -> Div {
    div()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
}

fn value_runs(value: &str, enabled: bool, p: &Palette) -> Vec<Div> {
    // Disabled rows mute variables too, not just the plain text around them.
    let (var_bg, var_fg) = if enabled {
        (p.accent.opacity(0.14), p.accent_hi)
    } else {
        (p.text_dim.opacity(0.12), p.text_dim)
    };
    split_vars(value)
        .into_iter()
        .map(|(seg, var)| {
            div()
                .flex_none()
                .when(var, |s| {
                    s.px(px(3.)).rounded(px(4.)).bg(var_bg).text_color(var_fg)
                })
                .child(SharedString::from(seg.to_string()))
        })
        .collect()
}

fn toggle(ix: usize, on: bool, p: &Palette, f: Option<IndexHandler>) -> impl IntoElement {
    div()
        .w(px(TOGGLE_COL_W))
        .flex_none()
        .flex()
        .items_center()
        .child(
            div()
                .id(("toggle", ix))
                .flex()
                .items_center()
                .justify_center()
                .size(px(14.))
                .rounded(px(4.))
                .cursor_pointer()
                .when(on, |s| s.bg(p.accent))
                .when(!on, |s| {
                    s.border_1()
                        .border_color(p.border)
                        .hover(|s| s.border_color(p.accent))
                })
                .when(on, |s| {
                    s.child(icon(IconName::Check, 10., gpui_kit::white()))
                })
                .when_some(f, |s, f| {
                    s.on_click(move |_, w, cx| {
                        cx.stop_propagation();
                        f(ix, w, cx)
                    })
                })
                .test_support(),
        )
}

impl RenderOnce for KeyValueTable {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();

        let header = div()
            .flex()
            .items_center()
            .h(px(26.))
            .px(px(8.))
            .border_b_1()
            .border_color(p.border)
            .text_size(px(10.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(p.text_dim)
            .child(div().w(px(TOGGLE_COL_W)).flex_none())
            .child(cell().w(px(KEY_W)).flex_none().child("KEY"))
            .child(cell().flex_1().child("VALUE"))
            .child(cell().w(px(DESC_W)).flex_none().child("DESCRIPTION"));

        let rows = self.rows.into_iter().enumerate().map(|(ix, row)| {
            let fg = if row.enabled { p.text } else { p.text_dim };
            div()
                .id(("row", ix))
                .flex()
                .items_center()
                .h(px(30.))
                .px(px(8.))
                .border_b_1()
                .border_color(p.border)
                .hover(|s| s.bg(p.surface_hi))
                .child(toggle(ix, row.enabled, &p, self.on_toggle.clone()))
                .child(
                    cell()
                        .w(px(KEY_W))
                        .flex_none()
                        .pr(px(8.))
                        .font_family(MONO)
                        .text_size(px(12.))
                        .text_color(fg)
                        .child(row.key),
                )
                .child(
                    cell()
                        .flex()
                        .flex_1()
                        .items_center()
                        .pr(px(8.))
                        .font_family(MONO)
                        .text_size(px(12.))
                        .text_color(fg)
                        .when(!row.enabled, |s| s.opacity(0.7))
                        .children(value_runs(&row.value, row.enabled, &p)),
                )
                .child(
                    cell()
                        .w(px(DESC_W))
                        .flex_none()
                        .text_size(px(11.5))
                        .text_color(p.text_dim)
                        .child(row.description),
                )
                .test_support()
        });

        let add = div()
            .id("add")
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(30.))
            .px(px(8.))
            .pl(px(8. + TOGGLE_COL_W))
            .text_size(px(12.))
            .text_color(p.text_dim)
            .cursor_pointer()
            .hover(|s| s.bg(p.surface_hi).text_color(p.text))
            .child(icon(IconName::Plus, 12., p.text_dim))
            .child("Add")
            .when_some(self.on_add, |s, f| s.on_click(move |_, w, cx| f(w, cx)))
            .test_support();

        div()
            .id(self.id)
            .flex()
            .flex_col()
            .w_full()
            .child(header)
            .children(rows)
            .child(add)
            .test_support()
    }
}
