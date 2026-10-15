use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::theme::ActivePalette;

/// Per-field Text / File choice for multipart form-data.
#[derive(IntoElement)]
pub struct FormFieldTypePicker {
    id: ElementId,
    file: bool,
    on_select: Rc<dyn Fn(bool, &mut Window, &mut App)>,
}

impl FormFieldTypePicker {
    pub fn new(
        id: impl Into<ElementId>,
        file: bool,
        on_select: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            file,
            on_select: Rc::new(on_select),
        }
    }
}

impl RenderOnce for FormFieldTypePicker {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .gap(px(1.))
            .p(px(2.))
            .rounded(px(5.))
            .bg(p.surface_hi)
            .children([("Text", false), ("File", true)].map(|(label, file)| {
                let selected = self.file == file;
                let on_select = self.on_select.clone();
                div()
                    .id(if file { "file" } else { "text" })
                    .px(px(6.))
                    .py(px(4.))
                    .rounded(px(4.))
                    .text_size(px(10.))
                    .text_color(if selected { p.text } else { p.text_dim })
                    .when(selected, |d| d.bg(p.surface))
                    .cursor_pointer()
                    .on_click(move |_, window, cx| on_select(file, window, cx))
                    .child(label)
                    .test_support()
            }))
            .test_support()
    }
}
