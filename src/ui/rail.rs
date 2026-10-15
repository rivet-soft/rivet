use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{ClickHandler, IconName, Tooltip, icon};
use crate::theme::ActivePalette;

/// Compact mode-rail button with a text tooltip.
#[derive(IntoElement)]
pub struct RailItem {
    id: ElementId,
    icon: IconName,
    label: SharedString,
    shortcut: SharedString,
    active: bool,
    on_click: Option<ClickHandler>,
}

impl RailItem {
    pub fn new(id: impl Into<ElementId>, icon: IconName, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            icon,
            label: label.into(),
            shortcut: "".into(),
            active: false,
            on_click: None,
        }
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = shortcut.into();
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for RailItem {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();
        let fg = if self.active { p.accent } else { p.text_dim };
        let tip = if self.shortcut.is_empty() {
            self.label.to_string()
        } else {
            format!("{} {}", self.label, self.shortcut)
        };
        div()
            .id(self.id)
            .flex()
            .items_center()
            .justify_center()
            .size(px(36.))
            .rounded(px(8.))
            .cursor_pointer()
            .when(self.active, |s| {
                s.bg(p.accent.opacity(0.16))
                    .border_l_2()
                    .border_color(p.accent)
            })
            .hover(|s| s.bg(p.surface_hi))
            .when_some(self.on_click, |s, f| {
                s.on_click(move |e, w, cx| f(e, w, cx))
            })
            .child(icon(self.icon, 18., fg))
            .tooltip(Tooltip::new(tip).build())
            .test_support()
    }
}
