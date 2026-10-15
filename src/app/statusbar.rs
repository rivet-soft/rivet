use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::Workspace;
use super::env_panel::env_color;
use crate::theme::{ActivePalette, MONO};
use crate::ui::StatusDot;

impl Workspace {
    pub(super) fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *cx.palette();
        let settings = &self.project.settings;
        let env = self.env();
        div()
            .id("status-bar")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(18.))
            .h(px(26.))
            .px(px(14.))
            .bg(p.surface)
            .border_t_1()
            .border_color(p.border)
            .font_family(MONO)
            .text_size(px(10.5))
            .text_color(p.text_dim)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        StatusDot::new(env.map(|e| env_color(e.color, &p)).unwrap_or(p.text_dim))
                            .size(6.),
                    )
                    .child(match env {
                        Some(e) => format!("{} · {}", e.name, e.base_url()),
                        None => "Без окружения".into(),
                    }),
            )
            .child(format!(
                "Proxy: {}",
                settings.proxy.as_deref().unwrap_or("off")
            ))
            .child(
                div()
                    .id("ssl-toggle")
                    .cursor_pointer()
                    .hover(|s| s.text_color(p.text))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.project.settings.ssl_verify = !this.project.settings.ssl_verify;
                        this.schedule_save(cx);
                        cx.notify();
                    }))
                    .when(!settings.ssl_verify, |s| s.text_color(p.warn))
                    .child(format!(
                        "SSL verify: {}",
                        if settings.ssl_verify { "on" } else { "off" }
                    ))
                    .test_support(),
            )
            .child(format!("Timeout: {} s", settings.timeout_ms / 1000))
            .child(div().flex_1())
            .when_some(self.notice.clone(), |d, n| {
                d.child(
                    div()
                        .id("notice")
                        .text_color(p.online)
                        .child(n)
                        .test_support(),
                )
            })
            .child(
                div()
                    .id("history-count")
                    .child(format!("History: {}", self.project.history.len()))
                    .test_support(),
            )
    }
}
