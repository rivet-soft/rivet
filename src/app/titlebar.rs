use gpui_kit::component::TitleBar;
use gpui_kit::*;

use super::Workspace;
use super::env_panel::env_color;
use crate::model::RailMode;
use crate::theme::{ActivePalette, MONO};
use crate::ui::{
    EnvPopover, IconName, Placement, Popover, RailItem, SearchField, StatusDot, icon, shortcut_hint,
};

impl Workspace {
    pub(super) fn render_titlebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *cx.palette();

        let version = self
            .spec
            .as_ref()
            .map(|s| format!("v{}", s.version))
            .unwrap_or_default();
        let project = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(13.))
                    .child(self.project.name.clone()),
            )
            .child(
                div()
                    .font_family(MONO)
                    .text_size(px(11.))
                    .text_color(p.text_dim)
                    .child(version),
            );

        let (env_name, env_dot) = match self.env() {
            Some(e) => (e.name.clone(), env_color(e.color, &p)),
            None => ("Без окружения".to_string(), p.text_dim),
        };
        let env_switch = div()
            .id("env-switch")
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(26.))
            .px(px(10.))
            .rounded(px(7.))
            .border_1()
            .border_color(p.border)
            .cursor_pointer()
            .hover(|s| s.bg(p.surface_hi))
            .on_click(cx.listener(|this, _, _, cx| {
                this.env_menu_open = !this.env_menu_open;
                cx.notify();
            }))
            .child(StatusDot::new(env_dot))
            .child(env_name)
            .child(icon(IconName::ChevronDown, 12., p.text_dim))
            .test_support();

        let view = cx.entity().downgrade();
        let envs: Vec<_> = self
            .project
            .environments
            .iter()
            .map(|e| {
                (
                    e.name.clone(),
                    e.base_url().to_string(),
                    env_color(e.color, &p),
                )
            })
            .collect();
        let env_switch = Popover::new("env-menu")
            .open(self.env_menu_open)
            .placement(Placement::BelowEnd)
            .width(320.)
            .trigger(env_switch)
            .child(
                EnvPopover::new("env-list", &envs)
                    .active(self.project.active_env)
                    .on_select({
                        let view = view.clone();
                        move |ix, window, cx| {
                            view.update(cx, |this, cx| this.set_env(ix, window, cx))
                                .ok();
                        }
                    }),
            )
            .on_dismiss(move |_, cx| {
                view.update(cx, |this, cx| {
                    this.env_menu_open = false;
                    cx.notify();
                })
                .ok();
            });


        TitleBar::new()
            .h(px(super::TITLEBAR_HEIGHT))
            .pl(px(if cfg!(target_os = "macos") { 88. } else { 14. }))
            .bg(p.surface)
            .border_color(p.border)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_between()
                    .pr(px(14.))
                    .child(project)
                    .child(
                        // `occlude` keeps the titlebar's drag area from swallowing
                        // clicks on Windows/Linux, where it is a native HTCAPTION.
                        div().id("command-search-area").occlude().w(px(420.)).child(
                            SearchField::new("command-search", "Search or run command…")
                                .kbd(shortcut_hint("⌘K", "Ctrl+K"))
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.open_palette(window, cx)
                                    }),
                                ),
                        ),
                    )
                    .child(div().id("env-switch-area").occlude().child(env_switch)),
            )
            .into_any_element()
    }

    pub(super) fn render_rail(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *cx.palette();
        let items = [
            (
                RailMode::Requests,
                IconName::SendHorizontal,
                "Запросы",
                shortcut_hint("⌘1", "Ctrl+1"),
            ),
            (
                RailMode::Environments,
                IconName::Layers,
                "Окружения",
                shortcut_hint("⌘2", "Ctrl+2"),
            ),
            (
                RailMode::Collections,
                IconName::Folder,
                "Коллекции",
                shortcut_hint("⌘4", "Ctrl+4"),
            ),
            (
                RailMode::History,
                IconName::RotateCcwClock,
                "История",
                shortcut_hint("⌘3", "Ctrl+3"),
            ),
        ];

        div()
            .id("mode-rail")
            .flex()
            .flex_col()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .w(px(super::MODE_RAIL_WIDTH))
            .pt(px(8.))
            .bg(p.surface)
            .border_r_1()
            .border_color(p.border)
            .children(items.into_iter().map(|(mode, ic, label, shortcut)| {
                RailItem::new(("rail", mode as usize), ic, label)
                    .active(self.rail == mode)
                    .shortcut(shortcut)
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.set_rail(mode, window, cx)),
                    )
            }))
            .child(div().flex_1())
            .child(
                RailItem::new("rail-settings", IconName::Settings, "Настройки")
                    .active(self.rail == RailMode::Settings)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.set_rail(RailMode::Settings, window, cx)
                    })),
            )
            .test_support()
            .into_any_element()
    }
}
