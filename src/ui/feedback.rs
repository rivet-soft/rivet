//! Feedback states for the response pane: spinner, indeterminate progress bar and the
//! empty / error / loading placeholders.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{Button, IconName, Kbd, icon};
// Titles and descriptions take part in the window text selection where the
// view mounts a `TextSelectionLayer` (e.g. the response pane); elsewhere they
// render as plain text.
use gpui_kit::base::SelectableText;
use crate::theme::ActivePalette;

fn child_id(parent: &ElementId, name: &'static str) -> ElementId {
    ElementId::NamedChild(Arc::new(parent.clone()), name.into())
}

/// Hint line with an optional inline key: `{kbd}` in `text` marks where the key goes,
/// otherwise the key is appended after the text.
fn hint_line(text: Option<SharedString>, kbd: Option<SharedString>, color: Hsla) -> Div {
    let row = div()
        .flex()
        .items_center()
        .gap(px(5.))
        .text_size(px(12.))
        .text_color(color);
    match (text, kbd) {
        (Some(text), Some(kbd)) => match text.split_once("{kbd}") {
            Some((before, after)) => row
                .when(!before.trim().is_empty(), |s| {
                    s.child(SharedString::from(before.trim_end().to_string()))
                })
                .child(Kbd::new(kbd))
                .when(!after.trim().is_empty(), |s| {
                    s.child(SharedString::from(after.trim_start().to_string()))
                }),
            None => row.child(text).child(Kbd::new(kbd)),
        },
        (Some(text), None) => row.child(text),
        (None, Some(kbd)) => row.child(Kbd::new(kbd)),
        (None, None) => row,
    }
}

/// Centered column used by every placeholder state.
fn centered(id: ElementId) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_col()
        .flex_1()
        .size_full()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .p(px(24.))
}

// ---------------------------------------------------------------------------------------

/// Rotating loader icon.
#[derive(IntoElement)]
pub struct Spinner {
    id: ElementId,
    size: f32,
    color: Option<Hsla>,
}

impl Spinner {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            size: 14.,
            color: None,
        }
    }

    /// 14px, inline with text (default).
    pub fn small(self) -> Self {
        self.size(14.)
    }

    /// 28px, for the loading placeholder.
    pub fn large(self) -> Self {
        self.size(28.)
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for Spinner {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = self.color.unwrap_or(cx.palette().text_dim);
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(self.size))
            .child(
                icon(IconName::LoaderCircle, self.size, color).with_animation(
                    "spin",
                    Animation::new(Duration::from_millis(900)).repeat(),
                    |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
                ),
            )
            .test_support()
    }
}

/// Thin 2px indeterminate bar: a 35%-wide segment sliding left → right (`relay-bar`).
#[derive(IntoElement)]
pub struct ProgressBar {
    id: ElementId,
    color: Option<Hsla>,
}

impl ProgressBar {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            color: None,
        }
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for ProgressBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = self.color.unwrap_or(cx.palette().accent);
        div()
            .id(self.id)
            .relative()
            .flex_none()
            .w_full()
            .h(px(2.))
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .top_0()
                    .h_full()
                    .w(relative(0.35))
                    .bg(color)
                    .with_animation(
                        "slide",
                        Animation::new(Duration::from_millis(1200))
                            .repeat()
                            .with_easing(ease_in_out),
                        // -35% (fully hidden left) → 100% (fully hidden right).
                        |bar, delta| bar.left(relative(-0.35 + 1.35 * delta)),
                    ),
            )
            .test_support()
    }
}

// ---------------------------------------------------------------------------------------

/// Placeholder for a pane with no content yet: "Ответ появится здесь".
#[derive(IntoElement)]
pub struct EmptyState {
    id: ElementId,
    title: SharedString,
    hint: Option<SharedString>,
    kbd: Option<SharedString>,
    icon: Option<IconName>,
}

impl EmptyState {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            hint: None,
            kbd: None,
            icon: None,
        }
    }

    /// Secondary line. Put `{kbd}` where the [`Kbd`] should appear, e.g.
    /// `"Нажмите {kbd}, чтобы отправить запрос"`; without it the key is appended.
    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn kbd(mut self, kbd: impl Into<SharedString>) -> Self {
        self.kbd = Some(kbd.into());
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();
        let has_hint = self.hint.is_some() || self.kbd.is_some();
        centered(self.id)
            .when_some(self.icon, |s, i| {
                s.child(icon(i, 28., p.text_dim.opacity(0.6)))
            })
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.text)
                    .child(SelectableText::new("title", self.title)),
            )
            .when(has_hint, |s| {
                s.child(hint_line(self.hint, self.kbd, p.text_dim))
            })
            .test_support()
    }
}

/// Failure placeholder: large danger-tinted icon, title, description and up to two actions.
#[derive(IntoElement)]
pub struct ErrorState {
    id: ElementId,
    title: SharedString,
    description: Option<SharedString>,
    icon: IconName,
    actions: Vec<Button>,
}

impl ErrorState {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            description: None,
            icon: IconName::Unplug,
            actions: Vec::new(),
        }
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = icon;
        self
    }

    /// Adds a ready-made button. Only the first two actions are rendered.
    pub fn action(mut self, button: Button) -> Self {
        self.actions.push(button);
        self
    }

    /// Primary (accent) action, id `NamedChild(<state id>, "primary")`.
    pub fn primary_action(
        self,
        label: impl Into<SharedString>,
        f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        let button = Button::new(child_id(&self.id, "primary"), label)
            .primary()
            .on_click(f);
        self.action(button)
    }

    /// Secondary (outlined) action, id `NamedChild(<state id>, "secondary")`.
    pub fn secondary_action(
        self,
        label: impl Into<SharedString>,
        f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        let button = Button::new(child_id(&self.id, "secondary"), label).on_click(f);
        self.action(button)
    }
}

impl RenderOnce for ErrorState {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();
        let mut actions = self.actions;
        actions.truncate(2);
        centered(self.id)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(56.))
                    .mb(px(4.))
                    .rounded_full()
                    .bg(p.danger.opacity(0.12))
                    .child(icon(self.icon, 26., p.danger)),
            )
            .child(
                div()
                    .text_size(px(13.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.text)
                    .child(SelectableText::new("title", self.title)),
            )
            .when_some(self.description, |s, d| {
                s.child(
                    div()
                        .max_w(px(380.))
                        .text_center()
                        .text_size(px(12.))
                        .text_color(p.text_dim)
                        .child(SelectableText::new("description", d)),
                )
            })
            .when(!actions.is_empty(), |s| {
                s.child(div().flex().gap(px(8.)).mt(px(6.)).children(actions))
            })
            .test_support()
    }
}

/// Request-in-flight placeholder: large spinner, "Ожидание ответа от {host} …", Esc hint.
#[derive(IntoElement)]
pub struct LoadingState {
    id: ElementId,
    host: SharedString,
}

impl LoadingState {
    pub fn new(id: impl Into<ElementId>, host: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            host: host.into(),
        }
    }
}

impl RenderOnce for LoadingState {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();
        let spinner_id = child_id(&self.id, "spinner");
        centered(self.id)
            .child(Spinner::new(spinner_id).large().color(p.accent))
            .child(
                div()
                    .mt(px(4.))
                    .text_size(px(13.))
                    .text_color(p.text)
                    .child(format!("Ожидание ответа от {} …", self.host)),
            )
            .child(hint_line(
                Some("{kbd} — отменить".into()),
                Some("Esc".into()),
                p.text_dim,
            ))
            .test_support()
    }
}
