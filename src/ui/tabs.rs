use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{ClickHandler, IconButton, IconName, MethodBadge, Placement, Popover};
use crate::model::Method;
use crate::theme::ActivePalette;

/// One request tab: method, title, unsaved dot, close button.
#[derive(IntoElement)]
pub struct Tab {
    id: ElementId,
    label: SharedString,
    method: Option<Method>,
    active: bool,
    dirty: bool,
    busy: bool,
    on_click: Option<ClickHandler>,
    on_close: Option<ClickHandler>,
    menu_open: bool,
    menu: Option<AnyElement>,
    on_menu_click: Option<ClickHandler>,
    on_context_menu: Option<Rc<dyn Fn(&MouseDownEvent, &mut Window, &mut App)>>,
    on_menu_dismiss: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    drag: Option<TabDrag>,
    on_drop: Option<Rc<dyn Fn(&TabDrag, &mut Window, &mut App)>>,
    hint: Option<SharedString>,
}

/// Drag payload of a tab being reordered: its index in the bar.
#[derive(Clone)]
pub struct TabDrag {
    pub index: usize,
    pub label: SharedString,
}

struct TabDragPreview {
    label: SharedString,
}

impl Render for TabDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *cx.palette();
        div()
            .px(px(12.))
            .py(px(7.))
            .rounded(px(6.))
            .bg(p.surface_hi)
            .border_1()
            .border_color(p.accent)
            .text_size(px(12.))
            .text_color(p.text)
            .child(self.label.clone())
    }
}

impl Tab {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            method: None,
            active: false,
            dirty: false,
            busy: false,
            on_click: None,
            on_close: None,
            menu_open: false,
            menu: None,
            on_menu_click: None,
            on_context_menu: None,
            on_menu_dismiss: None,
            drag: None,
            on_drop: None,
            hint: None,
        }
    }

    /// Hover hint (e.g. where the request is saved).
    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Makes the tab draggable as tab `index` (reordering).
    pub fn draggable(mut self, index: usize) -> Self {
        self.drag = Some(TabDrag { index, label: self.label.clone() });
        self
    }

    /// Another tab dropped onto this one.
    pub fn on_tab_drop(mut self, f: impl Fn(&TabDrag, &mut Window, &mut App) + 'static) -> Self {
        self.on_drop = Some(Rc::new(f));
        self
    }

    pub fn method(mut self, m: Method) -> Self {
        self.method = Some(m);
        self
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Unsaved changes: a dot next to the close button (like Postman).
    pub fn dirty(mut self, dirty: bool) -> Self {
        self.dirty = dirty;
        self
    }

    /// Work in progress (request sending, WebSocket open): a small spinner,
    /// shown instead of the unsaved dot.
    pub fn busy(mut self, busy: bool) -> Self {
        self.busy = busy;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }

    pub fn on_close(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Rc::new(f));
        self
    }

    pub fn menu(mut self, open: bool, content: impl IntoElement) -> Self {
        self.menu_open = open;
        self.menu = Some(content.into_any_element());
        self
    }

    pub fn on_menu_click(
        mut self,
        f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_menu_click = Some(Rc::new(f));
        self
    }

    pub fn on_context_menu(
        mut self,
        f: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_context_menu = Some(Rc::new(f));
        self
    }

    pub fn on_menu_dismiss(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_menu_dismiss = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Tab {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();
        let close_id = ElementId::NamedChild(std::sync::Arc::new(self.id.clone()), "close".into());
        let menu_button_id =
            ElementId::NamedChild(std::sync::Arc::new(self.id.clone()), "menu".into());
        let popover_id =
            ElementId::NamedChild(std::sync::Arc::new(self.id.clone()), "actions".into());
        let on_menu_click = self.on_menu_click.clone();

        let tab = div()
            .id(self.id)
            .group("tab")
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(7.))
            .h_full()
            .max_w(px(220.))
            .pl(px(12.))
            .pr(px(6.))
            .border_r_1()
            .border_color(p.border)
            .cursor_pointer()
            .bg(if self.active { p.bg } else { p.surface })
            .text_color(if self.active { p.text } else { p.text_dim })
            .when(!self.active, |s| s.hover(|s| s.bg(p.surface_hi)))
            .when_some(self.on_click, |s, f| {
                s.on_click(move |e, w, cx| f(e, w, cx))
            })
            .when_some(self.hint, |s, hint| s.tooltip(super::Tooltip::new(hint).build()))
            .when_some(self.drag, |s, drag| {
                s.on_drag(drag, |drag, _, _, cx| {
                    cx.new(|_| TabDragPreview { label: drag.label.clone() })
                })
            })
            .when_some(self.on_drop, |s, f| {
                s.drag_over::<TabDrag>(move |style, _, _, _| style.bg(p.accent.opacity(0.17)))
                    .on_drop(move |drag: &TabDrag, w, cx| f(drag, w, cx))
            })
            .when_some(self.on_context_menu, |s, f| {
                s.on_mouse_down(MouseButton::Right, move |event, window, cx| {
                    f(event, window, cx);
                })
            })
            // Accent line on top of the active tab.
            .when(self.active, |s| {
                s.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(2.))
                        .bg(p.accent),
                )
            })
            .when_some(self.method, |s, m| s.child(MethodBadge::new(m)))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(self.label),
            )
            .when(self.active, |s| {
                s.when_some(on_menu_click, |s, f| {
                    s.child(
                        IconButton::new(menu_button_id, IconName::Ellipsis)
                            .size(13.)
                            .on_click(move |e, w, cx| f(e, w, cx)),
                    )
                })
            })
            .child(
                div()
                    .w(px(20.))
                    .flex()
                    .justify_center()
                    .when(self.busy, |s| {
                        s.child(
                            super::Spinner::new(ElementId::NamedChild(
                                std::sync::Arc::new(close_id.clone()),
                                "busy".into(),
                            ))
                            .size(11.),
                        )
                    })
                    .when(self.dirty && !self.busy, |s| {
                        s.child(
                            div()
                                .id(ElementId::NamedChild(
                                    std::sync::Arc::new(close_id.clone()),
                                    "dirty".into(),
                                ))
                                .size(px(7.))
                                .rounded_full()
                                .bg(p.text)
                                .test_support(),
                        )
                    })
                    .when_some(self.on_close, |s, f| {
                        let btn = IconButton::new(close_id, IconName::X)
                            .size(12.)
                            .on_click(move |e, w, cx| f(e, w, cx));
                        // Close is always visible on the active tab, on hover otherwise.
                        s.child(
                            div()
                                .when(!self.active, |d| {
                                    d.invisible().group_hover("tab", |s| s.visible())
                                })
                                .child(btn),
                        )
                    }),
            )
            .test_support();

        let mut popover = Popover::new(popover_id)
            .open(self.menu_open)
            .fill_height(true)
            .placement(Placement::BelowEnd)
            .width(270.)
            .trigger(tab);
        if let Some(menu) = self.menu {
            popover = popover.child(menu);
        }
        if let Some(on_dismiss) = self.on_menu_dismiss {
            popover = popover.on_dismiss(move |w, cx| on_dismiss(w, cx));
        }
        popover
    }
}

/// Horizontal strip of tabs with a trailing `+` button.
#[derive(IntoElement)]
pub struct TabBar {
    id: ElementId,
    tabs: Vec<Tab>,
    on_new: Option<ClickHandler>,
}

impl TabBar {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            tabs: Vec::new(),
            on_new: None,
        }
    }

    pub fn tab(mut self, tab: Tab) -> Self {
        self.tabs.push(tab);
        self
    }

    pub fn tabs(mut self, tabs: impl IntoIterator<Item = Tab>) -> Self {
        self.tabs.extend(tabs);
        self
    }

    pub fn on_new(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_new = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for TabBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();
        let new_id = ElementId::NamedChild(std::sync::Arc::new(self.id.clone()), "new".into());
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .h(px(36.))
            .bg(p.surface)
            .border_b_1()
            .border_color(p.border)
            .overflow_x_scroll()
            .children(self.tabs)
            .when_some(self.on_new, |s, f| {
                s.child(div().px(px(6.)).child(
                    IconButton::new(new_id, IconName::Plus).on_click(move |e, w, cx| f(e, w, cx)),
                ))
            })
            .test_support()
    }
}
