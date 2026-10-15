//! Drag payload and preview for moving saved requests in the collections tree.
use crate::theme::ActivePalette;
use gpui_kit::*;

#[derive(Clone)]
pub struct SavedRequestDrag {
    pub request_id: String,
    pub label: String,
}

pub struct SavedRequestDragPreview {
    pub label: String,
}

impl Render for SavedRequestDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *cx.palette();
        div()
            .px(px(12.))
            .py(px(7.))
            .rounded(px(6.))
            .bg(p.surface_hi)
            .border_1()
            .border_color(p.accent)
            .text_color(p.text)
            .child(self.label.clone())
    }
}
