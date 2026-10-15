//! Actions shown from a request tab's context menu.

use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::theme::ActivePalette;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabAction {
    NewRequest,
    Duplicate,
    Close,
    ForceClose,
    CloseOthers,
    CloseAll,
    ForceCloseAll,
}

type SelectHandler = Rc<dyn Fn(TabAction, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct TabActionsMenu {
    id: ElementId,
    on_select: Option<SelectHandler>,
}

impl TabActionsMenu {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            on_select: None,
        }
    }

    pub fn on_select(
        mut self,
        handler: impl Fn(TabAction, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for TabActionsMenu {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();
        let actions = [
            (TabAction::NewRequest, "Новый запрос", "⌘T", "Ctrl+T"),
            (TabAction::Duplicate, "Дублировать вкладку", "", ""),
            (TabAction::Close, "Закрыть вкладку", "⌘W", "Ctrl+W"),
            (
                TabAction::ForceClose,
                "Закрыть принудительно",
                "⌥⌘W",
                "Ctrl+Alt+W",
            ),
            (TabAction::CloseOthers, "Закрыть остальные", "", ""),
            (TabAction::CloseAll, "Закрыть все вкладки", "", ""),
            (
                TabAction::ForceCloseAll,
                "Закрыть все принудительно",
                "",
                "",
            ),
        ];
        div()
            .id(self.id)
            .flex()
            .flex_col()
            .p(px(5.))
            .children(actions.into_iter().map(|(action, label, mac, other)| {
                let handler = self.on_select.clone();
                div()
                    .when(action == TabAction::Close, |d| {
                        d.mt(px(4.)).border_t_1().border_color(p.border).pt(px(4.))
                    })
                    .child(
                        div()
                            .id(("tab-action", action as usize))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(20.))
                            .h(px(30.))
                            .px(px(10.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|d| d.bg(p.surface_hi))
                            .on_click(move |_, window, cx| {
                                if let Some(handler) = &handler {
                                    handler(action, window, cx);
                                }
                            })
                            .child(label)
                            .child(div().text_size(px(10.)).text_color(p.text_dim).child(
                                if cfg!(target_os = "macos") {
                                    mac
                                } else {
                                    other
                                },
                            ))
                            .test_support(),
                    )
            }))
            .test_support()
    }
}
