use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::theme::ActivePalette;

/// Inline segmented switch: `Pretty | Raw | Preview`, `JSON | Form | …`.
#[derive(IntoElement)]
pub struct Segmented {
    id: ElementId,
    items: Vec<SharedString>,
    selected: usize,
    on_select: Option<Rc<dyn Fn(usize, &mut Window, &mut App)>>,
}

impl Segmented {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            items: Vec::new(),
            selected: 0,
            on_select: None,
        }
    }

    pub fn items<S: Into<SharedString>>(mut self, items: impl IntoIterator<Item = S>) -> Self {
        self.items = items.into_iter().map(Into::into).collect();
        self
    }

    pub fn selected(mut self, ix: usize) -> Self {
        self.selected = ix;
        self
    }

    pub fn on_select(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Segmented {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .self_start()
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(7.))
            .bg(p.surface_hi)
            .children(self.items.into_iter().enumerate().map(|(ix, label)| {
                let active = ix == self.selected;
                let on_select = self.on_select.clone();
                div()
                    .id(ix)
                    .px(px(10.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .text_size(px(11.5))
                    .cursor_pointer()
                    .text_color(if active { p.text } else { p.text_dim })
                    .when(active, |s| s.bg(p.surface).shadow_xs())
                    .when(!active, |s| s.hover(|s| s.text_color(p.text)))
                    .when_some(on_select, |s, f| s.on_click(move |_, w, cx| f(ix, w, cx)))
                    .child(label)
                    .test_support()
            }))
            .test_support()
    }
}
