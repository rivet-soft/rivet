//! Minimal view proving the toolchain, window, theme and input all work.

use gpui_kit::component::TitleBar;
use gpui_kit::*;

use crate::theme::{ActivePalette, MONO, Palette};

pub struct Smoke {
    pub clicks: usize,
}

impl Render for Smoke {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *cx.palette();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.bg)
            .text_color(p.text)
            .child(
                TitleBar::new()
                    .bg(p.surface)
                    .border_color(p.border)
                    .child("Rivet"),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .id("ping")
                            .px(px(14.))
                            .py(px(6.))
                            .rounded(px(7.))
                            .bg(p.accent)
                            .text_color(gpui_kit::white())
                            .cursor_pointer()
                            .hover(|s| s.bg(p.accent_hi))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.clicks += 1;
                                cx.notify();
                            }))
                            .child("Send")
                            .test_support(),
                    )
                    .child(
                        div()
                            .id("clicks")
                            .font_family(MONO)
                            .text_color(p.text_dim)
                            .child(format!("clicks: {}", self.clicks)),
                    )
                    .child(
                        div()
                            .id("toggle-theme")
                            .cursor_pointer()
                            .text_color(p.accent)
                            .on_click(|_, _, cx| Palette::toggle(cx))
                            .child("Toggle theme")
                            .test_support(),
                    ),
            )
    }
}
