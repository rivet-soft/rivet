//! Floating surfaces: an anchored [`Popover`], the Prod-delete [`ConfirmPopover`],
//! the environment picker [`EnvPopover`] and palette-styled [`Tooltip`]s.
//!
//! Popovers are controlled: the owning view keeps `open` and flips it from
//! `on_dismiss` (click outside, Escape) or from the content's own callbacks.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{Button, IconName, StatusDot, icon};
use crate::theme::{ActivePalette, MONO};

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;
type SelectHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// Where the panel opens relative to its trigger.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Placement {
    /// Under the trigger, left edges aligned.
    #[default]
    Below,
    /// Over the trigger, left edges aligned.
    Above,
    /// Under the trigger, right edges aligned.
    BelowEnd,
    /// Over the trigger, right edges aligned.
    AboveEnd,
    /// Right of the trigger, top edges aligned (grows down).
    Right,
    /// Right of the trigger, bottom edges aligned (grows up).
    RightEnd,
}

/// Per-element state kept across frames: focus for Escape handling, plus the
/// trigger bounds so clicking the trigger doesn't count as "outside".
struct PopoverState {
    focus: FocusHandle,
    previous_focus: Option<FocusHandle>,
    was_open: bool,
    trigger_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

/// Anchored floating panel: surface background, border, 10px radius, soft shadow.
///
/// Renders `trigger` in place and, while `open`, the content in a deferred
/// layer above everything else, snapped inside the window.
#[derive(IntoElement)]
pub struct Popover {
    id: ElementId,
    open: bool,
    fill_width: bool,
    fill_height: bool,
    placement: Placement,
    gap: Pixels,
    width: Option<Pixels>,
    trigger: Option<AnyElement>,
    content: Vec<AnyElement>,
    on_dismiss: Option<Handler>,
}

impl Popover {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            open: false,
            fill_width: false,
            fill_height: false,
            placement: Placement::default(),
            gap: px(6.),
            width: None,
            trigger: None,
            content: Vec::new(),
            on_dismiss: None,
        }
    }

    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Let the trigger wrapper take the remaining width in a flex row.
    pub fn fill_width(mut self, fill: bool) -> Self {
        self.fill_width = fill;
        self
    }

    /// Stretch the trigger wrapper to the parent's height.
    pub fn fill_height(mut self, fill: bool) -> Self {
        self.fill_height = fill;
        self
    }

    pub fn placement(mut self, placement: Placement) -> Self {
        self.placement = placement;
        self
    }

    /// Distance between trigger and panel (default 6px).
    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = px(gap);
        self
    }

    /// Fixed panel width; otherwise it sizes to content.
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(px(width));
        self
    }

    pub fn trigger(mut self, trigger: impl IntoElement) -> Self {
        self.trigger = Some(trigger.into_any_element());
        self
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.content.push(child.into_any_element());
        self
    }

    /// Fired on mouse-down outside the panel (the trigger excluded) and on Escape.
    pub fn on_dismiss(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Popover {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();
        let state = window.use_keyed_state(self.id.clone(), cx, |_, cx| PopoverState {
            focus: cx.focus_handle(),
            previous_focus: None,
            was_open: false,
            trigger_bounds: Rc::default(),
        });

        // Take focus on open so Escape reaches us; give it back on close.
        let open = self.open;
        let (was_open, focus, trigger_bounds) = state.update(cx, |s, _| {
            let was_open = std::mem::replace(&mut s.was_open, open);
            (was_open, s.focus.clone(), s.trigger_bounds.clone())
        });
        if open && !was_open {
            let previous = window.focused(cx);
            state.update(cx, |s, _| s.previous_focus = previous);
            focus.focus(window, cx);
        } else if !open && was_open {
            let previous = state.update(cx, |s, _| s.previous_focus.take());
            if focus.contains_focused(window, cx)
                && let Some(previous) = previous
            {
                previous.focus(window, cx);
            }
        }

        let (corner, below) = match self.placement {
            Placement::Below => (Anchor::TopLeft, true),
            Placement::BelowEnd => (Anchor::TopRight, true),
            Placement::Above => (Anchor::BottomLeft, false),
            Placement::AboveEnd => (Anchor::BottomRight, false),
            Placement::Right => (Anchor::TopLeft, true),
            Placement::RightEnd => (Anchor::BottomLeft, false),
        };
        let end = matches!(self.placement, Placement::BelowEnd | Placement::AboveEnd);
        let side = matches!(self.placement, Placement::Right | Placement::RightEnd);

        let panel = self.open.then(|| {
            let dismiss_out = self.on_dismiss.clone();
            let dismiss_esc = self.on_dismiss.clone();
            let bounds = trigger_bounds.clone();
            div()
                .id("panel")
                .track_focus(&focus)
                .occlude()
                .flex()
                .flex_col()
                .when_some(self.width, |s, w| s.w(w))
                .when(side, |s| s.ml(self.gap))
                .when(below && !side, |s| s.mt(self.gap))
                .when(!below && !side, |s| s.mb(self.gap))
                .bg(p.surface)
                .border_1()
                .border_color(p.border)
                .rounded(px(10.))
                .shadow(vec![BoxShadow {
                    color: black().opacity(0.28),
                    offset: point(px(0.), px(8.)),
                    blur_radius: px(24.),
                    spread_radius: px(0.),
                    inset: false,
                }])
                .text_color(p.text)
                .on_mouse_down_out(move |_, window, cx| {
                    let on_trigger = bounds
                        .get()
                        .is_some_and(|b| b.contains(&window.mouse_position()));
                    if !on_trigger && let Some(f) = &dismiss_out {
                        f(window, cx)
                    }
                })
                .on_key_down(move |e, window, cx| {
                    if e.keystroke.key == "escape" {
                        cx.stop_propagation();
                        if let Some(f) = &dismiss_esc {
                            f(window, cx)
                        }
                    }
                })
                .children(self.content)
                .test_support()
        });

        let layer = panel.map(|panel| {
            // A zero-size marker at the trigger's top/bottom edge gives `anchored`
            // its origin; the anchor corner then decides which way the panel grows.
            div()
                .absolute()
                .when(side, |s| s.left_full())
                .when(side && below, |s| s.top_0())
                .when(side && !below, |s| s.bottom_0())
                .when(!side && below, |s| s.top_full())
                .when(!side && !below, |s| s.bottom_full())
                .when(!side && end, |s| s.right_0())
                .when(!side && !end, |s| s.left_0())
                .child(
                    deferred(anchored().anchor(corner).snap_to_window().child(panel)).priority(1),
                )
        });

        let has_trigger = self.trigger.is_some();
        div()
            .on_children_prepainted(move |children, _, _| {
                trigger_bounds.set(children.first().copied().filter(|_| has_trigger));
            })
            .id(self.id)
            .relative()
            .flex_none()
            .when(self.fill_width, |s| s.flex_1().min_w_0())
            .when(self.fill_height, |s| s.h_full())
            .when_some(self.trigger, |s, t| s.child(t))
            .children(layer)
    }
}

/// "DELETE в Prod · подтверждение": danger mark, question, target URL,
/// `Отмена` / `Отправить в Prod`. Dismissing counts as cancel.
#[derive(IntoElement)]
pub struct ConfirmPopover {
    id: ElementId,
    open: bool,
    placement: Placement,
    title: SharedString,
    url: SharedString,
    confirm_label: SharedString,
    trigger: Option<AnyElement>,
    on_cancel: Option<Handler>,
    on_confirm: Option<Handler>,
}

impl ConfirmPopover {
    pub fn new(id: impl Into<ElementId>, url: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            open: false,
            placement: Placement::BelowEnd,
            title: "Вы отправляете DELETE в Production. Продолжить?".into(),
            url: url.into(),
            confirm_label: "Отправить в Prod".into(),
            trigger: None,
            on_cancel: None,
            on_confirm: None,
        }
    }

    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    pub fn placement(mut self, placement: Placement) -> Self {
        self.placement = placement;
        self
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = title.into();
        self
    }

    pub fn confirm_label(mut self, label: impl Into<SharedString>) -> Self {
        self.confirm_label = label.into();
        self
    }

    pub fn trigger(mut self, trigger: impl IntoElement) -> Self {
        self.trigger = Some(trigger.into_any_element());
        self
    }

    /// `Отмена`, click outside and Escape.
    pub fn on_cancel(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_cancel = Some(Rc::new(f));
        self
    }

    pub fn on_confirm(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_confirm = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for ConfirmPopover {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();
        let cancel = self.on_cancel.clone();
        let dismiss = self.on_cancel;
        let confirm = self.on_confirm;

        let body = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(14.))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(px(22.))
                            .rounded_full()
                            .bg(p.danger.opacity(0.16))
                            .text_color(p.danger)
                            .text_size(px(12.))
                            .font_weight(FontWeight::BOLD)
                            .child("!"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .line_height(px(17.))
                                    .child(self.title),
                            )
                            .child(
                                div()
                                    .px(px(7.))
                                    .py(px(3.))
                                    .rounded(px(5.))
                                    .bg(p.bg)
                                    .font_family(MONO)
                                    .text_size(px(11.))
                                    .text_color(p.text_dim)
                                    .truncate()
                                    .child(self.url),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(Button::new("cancel", "Отмена").on_click(move |_, w, cx| {
                        if let Some(f) = &cancel {
                            f(w, cx)
                        }
                    }))
                    .child(
                        Button::new("confirm", self.confirm_label)
                            .danger()
                            .on_click(move |_, w, cx| {
                                if let Some(f) = &confirm {
                                    f(w, cx)
                                }
                            }),
                    ),
            );

        Popover::new(self.id)
            .open(self.open)
            .placement(self.placement)
            .width(320.)
            .when_some(self.trigger, |s, t| s.trigger(t))
            .when_some(dismiss, |s, f| s.on_dismiss(move |w, cx| f(w, cx)))
            .child(body)
    }
}

/// Environment picker content: colour dot, name, base URL, check on the active one.
/// Put it inside a [`Popover`]; rows are `("env", index)`.
#[derive(IntoElement)]
pub struct EnvPopover {
    id: ElementId,
    envs: Vec<(SharedString, SharedString, Hsla)>,
    active: Option<usize>,
    on_select: Option<SelectHandler>,
}

impl EnvPopover {
    pub fn new<N: AsRef<str>, U: AsRef<str>>(
        id: impl Into<ElementId>,
        envs: &[(N, U, Hsla)],
    ) -> Self {
        Self {
            id: id.into(),
            envs: envs
                .iter()
                .map(|(n, u, c)| {
                    (
                        n.as_ref().to_owned().into(),
                        u.as_ref().to_owned().into(),
                        *c,
                    )
                })
                .collect(),
            active: None,
            on_select: None,
        }
    }

    pub fn active(mut self, index: usize) -> Self {
        self.active = Some(index);
        self
    }

    pub fn on_select(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for EnvPopover {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();
        let on_select = self.on_select;
        let active = self.active;

        div()
            .id(self.id)
            .flex()
            .flex_col()
            .min_w(px(240.))
            .p(px(4.))
            .gap(px(1.))
            .children(
                self.envs
                    .into_iter()
                    .enumerate()
                    .map(|(i, (name, url, color))| {
                        let is_active = active == Some(i);
                        let on_select = on_select.clone();
                        div()
                            .id(("env", i))
                            .flex()
                            .items_center()
                            .gap(px(9.))
                            .px(px(8.))
                            .py(px(6.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .when(is_active, |s| s.bg(p.surface_hi))
                            .hover(|s| s.bg(p.surface_hi))
                            .when_some(on_select, |s, f| s.on_click(move |_, w, cx| f(i, w, cx)))
                            .child(StatusDot::new(color).size(8.))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(p.text)
                                            .child(name),
                                    )
                                    .child(
                                        div()
                                            .font_family(MONO)
                                            .text_size(px(10.5))
                                            .text_color(p.text_dim)
                                            .truncate()
                                            .child(url),
                                    ),
                            )
                            .child(
                                div().flex_none().size(px(14.)).when(is_active, |s| {
                                    s.child(icon(IconName::Check, 14., p.accent))
                                }),
                            )
                            .test_support()
                    }),
            )
    }
}

/// Palette-styled hover hint for gpui's `.tooltip(..)`.
///
/// ```ignore
/// div().id("qty").tooltip(Tooltip::new("quantity").part("integer").part("required").part("min: 1").build())
/// ```
#[derive(Clone)]
pub struct Tooltip {
    title: SharedString,
    parts: Vec<SharedString>,
    mono: bool,
}

impl Tooltip {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            parts: Vec::new(),
            mono: false,
        }
    }

    /// Dimmed segment appended after ` · `.
    pub fn part(mut self, part: impl Into<SharedString>) -> Self {
        self.parts.push(part.into());
        self
    }

    /// Render in the monospace font (schema hints, paths).
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    /// Builder for `StatefulInteractiveElement::tooltip`.
    pub fn build(self) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        move |_, cx| cx.new(|_| self.clone()).into()
    }

    /// Plain one-line hint.
    pub fn text(
        text: impl Into<SharedString>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        Self::new(text).build()
    }
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *cx.palette();
        div()
            .id("tooltip")
            .flex()
            .items_center()
            .gap(px(5.))
            .px(px(8.))
            .py(px(4.))
            .rounded(px(6.))
            .bg(p.surface_hi)
            .border_1()
            .border_color(p.border)
            .shadow(vec![BoxShadow {
                color: black().opacity(0.22),
                offset: point(px(0.), px(4.)),
                blur_radius: px(12.),
                spread_radius: px(0.),
                inset: false,
            }])
            .text_size(px(11.))
            .when(self.mono, |s| s.font_family(MONO))
            .child(div().text_color(p.text).child(self.title.clone()))
            .children(self.parts.iter().flat_map(|part| {
                [
                    div().text_color(p.text_dim.opacity(0.6)).child("·"),
                    div().text_color(p.text_dim).child(part.clone()),
                ]
            }))
            .test_support()
    }
}
