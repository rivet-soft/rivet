use std::rc::Rc;

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;

use super::Segmented;
use crate::theme::ActivePalette;

/// Request-level redirect controls. Empty limit and "По умолчанию" inherit app settings.
#[derive(IntoElement)]
pub struct RequestRedirectSettings {
    follow: Option<bool>,
    default_follow: bool,
    default_limit: u32,
    limit: Entity<InputState>,
    on_follow: Rc<dyn Fn(Option<bool>, &mut Window, &mut App)>,
}

impl RequestRedirectSettings {
    pub fn new(
        follow: Option<bool>,
        default_follow: bool,
        default_limit: u32,
        limit: Entity<InputState>,
        on_follow: impl Fn(Option<bool>, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            follow,
            default_follow,
            default_limit,
            limit,
            on_follow: Rc::new(on_follow),
        }
    }
}

impl RenderOnce for RequestRedirectSettings {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();
        let selected = match self.follow {
            None => 0,
            Some(true) => 1,
            Some(false) => 2,
        };
        let on_follow = self.on_follow.clone();
        div()
            .id("request-redirect-settings")
            .flex()
            .flex_col()
            .gap(px(16.))
            .p(px(18.))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Редиректы"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(p.text_dim)
                    .child(format!(
                        "Настройка приложения: {} · максимум {} переходов",
                        if self.default_follow {
                            "включены"
                        } else {
                            "выключены"
                        },
                        self.default_limit
                    )),
            )
            .child(
                Segmented::new("request-follow-redirects")
                    .items(["По умолчанию", "Включено", "Выключено"])
                    .selected(selected)
                    .on_select(move |index, window, cx| {
                        on_follow(
                            match index {
                                1 => Some(true),
                                2 => Some(false),
                                _ => None,
                            },
                            window,
                            cx,
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(div().text_size(px(12.)).child("Максимум переходов"))
                    .child(
                        div().w(px(160.)).child(
                            Input::new(&self.limit)
                                .id("request-redirect-limit")
                                .h(px(30.)),
                        ),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(p.text_dim)
                    .child("Оставьте поле пустым для значения приложения. Допустимо от 1 до 50."),
            )
            .test_support()
    }
}
