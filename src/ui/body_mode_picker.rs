use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::theme::ActivePalette;

/// Body format selector that wraps options instead of hiding them in a narrow pane.
#[derive(IntoElement)]
pub struct BodyModePicker {
    selected: usize,
    on_select: Rc<dyn Fn(usize, &mut Window, &mut App)>,
}

impl BodyModePicker {
    pub fn new(
        selected: usize,
        on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            selected,
            on_select: Rc::new(on_select),
        }
    }
}

impl RenderOnce for BodyModePicker {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();
        let labels = [
            "none",
            "form-data",
            "x-www-form-urlencoded",
            "raw JSON",
            "raw Text",
            "binary",
        ];
        div()
            .id("body-mode")
            .flex()
            .flex_wrap()
            .flex_shrink(1.)
            .min_w_0()
            .max_w_full()
            .self_start()
            .gap(px(2.))
            .p(px(2.))
            .rounded(px(7.))
            .bg(p.surface_hi)
            .children(labels.into_iter().enumerate().map(|(ix, label)| {
                let selected = ix == self.selected;
                let on_select = self.on_select.clone();
                div()
                    .id(ix)
                    .flex_none()
                    .px(px(10.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .text_size(px(11.5))
                    .cursor_pointer()
                    .text_color(if selected { p.text } else { p.text_dim })
                    .when(selected, |d| d.bg(p.surface).shadow_xs())
                    .when(!selected, |d| d.hover(|d| d.text_color(p.text)))
                    .on_click(move |_, window, cx| on_select(ix, window, cx))
                    .child(label)
                    .test_support()
            }))
            .test_support()
    }
}
