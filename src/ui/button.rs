use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{ClickHandler, IconName, Kbd, icon};
use crate::theme::{ActivePalette, Palette};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonVariant {
    /// Filled accent: Send, primary confirmations.
    Primary,
    /// Outlined surface button.
    #[default]
    Secondary,
    /// Text-only, background on hover.
    Ghost,
    /// Filled danger: "Отправить в Prod".
    Danger,
}

struct Colors {
    bg: Option<Hsla>,
    hover: Hsla,
    fg: Hsla,
    border: Option<Hsla>,
}

impl ButtonVariant {
    fn colors(self, p: &Palette) -> Colors {
        let white: Hsla = gpui_kit::white();
        match self {
            Self::Primary => Colors {
                bg: Some(p.accent),
                hover: p.accent_hi,
                fg: white,
                border: None,
            },
            Self::Danger => Colors {
                bg: Some(p.danger),
                hover: p.danger.opacity(0.85),
                fg: white,
                border: None,
            },
            Self::Secondary => Colors {
                bg: Some(p.surface),
                hover: p.surface_hi,
                fg: p.text,
                border: Some(p.border),
            },
            Self::Ghost => Colors {
                bg: None,
                hover: p.surface_hi,
                fg: p.text_dim,
                border: None,
            },
        }
    }
}

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    variant: ButtonVariant,
    icon: Option<IconName>,
    kbd: Option<SharedString>,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            variant: ButtonVariant::default(),
            icon: None,
            kbd: None,
            disabled: false,
            on_click: None,
        }
    }

    pub fn variant(mut self, v: ButtonVariant) -> Self {
        self.variant = v;
        self
    }

    pub fn primary(self) -> Self {
        self.variant(ButtonVariant::Primary)
    }

    pub fn danger(self) -> Self {
        self.variant(ButtonVariant::Danger)
    }

    pub fn ghost(self) -> Self {
        self.variant(ButtonVariant::Ghost)
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn kbd(mut self, kbd: impl Into<SharedString>) -> Self {
        self.kbd = Some(kbd.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();
        let c = self.variant.colors(&p);
        let hover = c.hover;
        let filled = matches!(self.variant, ButtonVariant::Primary | ButtonVariant::Danger);

        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .px(px(12.))
            .rounded(px(7.))
            .text_size(px(12.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(c.fg)
            .when_some(c.bg, |s, bg| s.bg(bg))
            .when_some(c.border, |s, b| s.border_1().border_color(b))
            .when(self.disabled, |s| s.opacity(0.5))
            .when(!self.disabled, |s| {
                s.cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .when_some(self.on_click, |s, f| {
                        s.on_click(move |e, w, cx| f(e, w, cx))
                    })
            })
            .when_some(self.icon, |s, i| s.child(icon(i, 13., c.fg)))
            .child(self.label)
            .when_some(self.kbd, |s, k| {
                // Filled buttons show the hint as faint text rather than a bordered key.
                if filled {
                    s.child(div().text_size(px(10.5)).opacity(0.75).child(k))
                } else {
                    s.child(Kbd::new(k))
                }
            })
            .test_support()
    }
}

/// Square icon-only button (close tab, overflow menu, eye toggle).
#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    icon: IconName,
    size: f32,
    color: Option<Hsla>,
    on_click: Option<ClickHandler>,
}

impl IconButton {
    pub fn new(id: impl Into<ElementId>, icon: IconName) -> Self {
        Self {
            id: id.into(),
            icon,
            size: 14.,
            color: None,
            on_click: None,
        }
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for IconButton {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();
        let box_size = px(self.size + 8.);
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(box_size)
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|s| s.bg(p.surface_hi))
            .when_some(self.on_click, |s, f| {
                s.on_click(move |e, w, cx| {
                    // Don't let e.g. a tab's close button also activate the tab.
                    cx.stop_propagation();
                    f(e, w, cx)
                })
            })
            .child(icon(self.icon, self.size, self.color.unwrap_or(p.text_dim)))
            .test_support()
    }
}
