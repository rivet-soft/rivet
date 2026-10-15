use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::theme::ActivePalette;

pub const PANE_SPLITTER_WIDTH: f32 = 6.;
pub const PANE_SPLITTER_HEIGHT: f32 = 6.;

type DragStartHandler = Rc<dyn Fn(&MouseDownEvent, &mut Window, &mut App)>;

/// A narrow visible rule with a wider horizontal resize target.
#[derive(IntoElement)]
pub struct PaneSplitter {
    id: ElementId,
    active: bool,
    on_drag_start: Option<DragStartHandler>,
}

impl PaneSplitter {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            active: false,
            on_drag_start: None,
        }
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn on_drag_start(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_drag_start = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for PaneSplitter {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .justify_center()
            .w(px(PANE_SPLITTER_WIDTH))
            .h_full()
            .cursor(CursorStyle::ResizeLeftRight)
            .hover(|d| d.bg(p.accent.opacity(0.10)))
            .when_some(self.on_drag_start, |d, handler| {
                d.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                    handler(event, window, cx)
                })
            })
            .child(
                div()
                    .w(px(1.))
                    .h_full()
                    .bg(if self.active { p.accent } else { p.border }),
            )
            .test_support()
    }
}

/// A horizontal handle for resizing two vertically stacked panes.
#[derive(IntoElement)]
pub struct HorizontalPaneSplitter {
    id: ElementId,
    active: bool,
    on_drag_start: Option<DragStartHandler>,
}

impl HorizontalPaneSplitter {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            active: false,
            on_drag_start: None,
        }
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn on_drag_start(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_drag_start = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for HorizontalPaneSplitter {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .h(px(PANE_SPLITTER_HEIGHT))
            .w_full()
            .cursor(CursorStyle::ResizeUpDown)
            .hover(|d| d.bg(p.accent.opacity(0.10)))
            .when_some(self.on_drag_start, |d, handler| {
                d.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                    handler(event, window, cx)
                })
            })
            .child(
                div()
                    .h(px(1.))
                    .w_full()
                    .bg(if self.active { p.accent } else { p.border }),
            )
            .test_support()
    }
}
