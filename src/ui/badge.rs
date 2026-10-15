use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::model::Method;
use crate::theme::{ActivePalette, MONO};

/// Keyboard shortcut hint, e.g. `⌘K`.
#[derive(IntoElement)]
pub struct Kbd(SharedString);

impl Kbd {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self(text.into())
    }
}

impl RenderOnce for Kbd {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = cx.palette();
        div()
            .flex_none()
            .px(px(5.))
            .py(px(1.))
            .rounded(px(4.))
            .border_1()
            .border_color(p.border)
            .font_family(MONO)
            .text_size(px(10.))
            .text_color(p.text_dim)
            .child(self.0)
    }
}

/// Coloured HTTP method label used in the tree, tabs and request line.
#[derive(IntoElement)]
pub struct MethodBadge {
    method: Method,
    width: Option<Pixels>,
    full: bool,
}

impl MethodBadge {
    pub fn new(method: Method) -> Self {
        Self {
            method,
            width: None,
            full: false,
        }
    }

    /// Fixed column width so labels line up in lists.
    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(px(w));
        self
    }

    /// `DELETE` instead of the compact `DEL`.
    pub fn full(mut self) -> Self {
        self.full = true;
        self
    }
}

impl RenderOnce for MethodBadge {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = cx.palette();
        let label = if self.full {
            self.method.full_label()
        } else {
            self.method.label()
        };
        div()
            .flex_none()
            .when_some(self.width, |s, w| s.w(w))
            .font_family(MONO)
            .text_size(px(10.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(self.method.color(p))
            .child(label)
    }
}

/// Small filled circle: environment colour, online state.
#[derive(IntoElement)]
pub struct StatusDot {
    color: Hsla,
    size: Pixels,
}

impl StatusDot {
    pub fn new(color: Hsla) -> Self {
        Self {
            color,
            size: px(7.),
        }
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = px(size);
        self
    }
}

impl RenderOnce for StatusDot {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .flex_none()
            .size(self.size)
            .rounded_full()
            .bg(self.color)
    }
}

/// Tinted rounded label: `201 Created`, counters, `Linked to spec`.
#[derive(IntoElement)]
pub struct Pill {
    label: SharedString,
    color: Option<Hsla>,
    mono: bool,
}

impl Pill {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            color: None,
            mono: false,
        }
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }
}

impl RenderOnce for Pill {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = cx.palette();
        let fg = self.color.unwrap_or(p.text_dim);
        div()
            .flex_none()
            .px(px(7.))
            .py(px(1.))
            .rounded(px(10.))
            .bg(fg.opacity(0.14))
            .text_color(fg)
            .text_size(px(10.5))
            .font_weight(FontWeight::MEDIUM)
            .when(self.mono, |s| s.font_family(MONO))
            .child(self.label)
    }
}
