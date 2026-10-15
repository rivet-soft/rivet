use std::time::Duration;

use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    App, Context, Entity, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, TestAppContext, TestSupportExt as _,
    VisualTestContext, WeakEntity, Window, div, px, rgb,
};
use rivet::theme::Palette;
use rivet::ui::{Button, ConfirmPopover, EnvPopover, Popover, Tooltip};

#[derive(Default)]
struct Harness {
    open: bool,
    env_open: bool,
    active_env: usize,
    dismissed: usize,
    cancelled: usize,
    confirmed: usize,
}

/// `Fn(&mut Window, &mut App)` callback mutating the harness.
fn upd(
    view: &WeakEntity<Harness>,
    f: fn(&mut Harness),
) -> impl Fn(&mut Window, &mut App) + 'static {
    let view = view.clone();
    move |_, cx| {
        view.update(cx, |v, cx| {
            f(v);
            cx.notify();
        })
        .ok();
    }
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let envs = [
            ("Local", "http://localhost:8080", Hsla::from(rgb(0x2fa98c))),
            (
                "Staging",
                "https://staging.shop.com",
                Hsla::from(rgb(0xe5a050)),
            ),
            (
                "Production",
                "https://api.shop.com",
                Hsla::from(rgb(0xff6b6b)),
            ),
        ];
        let view = cx.entity().downgrade();
        let toggle = upd(&view, |v| v.open = !v.open);
        let toggle_env = upd(&view, |v| v.env_open = !v.env_open);
        let select_view = view.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(40.))
            .p(px(20.))
            .child(div().id("outside").w(px(100.)).h(px(20.)).test_support())
            .child(
                ConfirmPopover::new("confirm-pop", "https://api.shop.com/v2/orders/ord_8f3k29")
                    .open(self.open)
                    .trigger(
                        Button::new("send", "Send")
                            .primary()
                            .on_click(move |_, w, cx| toggle(w, cx)),
                    )
                    .on_cancel(upd(&view, |v| {
                        v.open = false;
                        v.cancelled += 1;
                    }))
                    .on_confirm(upd(&view, |v| {
                        v.open = false;
                        v.confirmed += 1;
                    })),
            )
            .child(
                Popover::new("env-pop")
                    .open(self.env_open)
                    .trigger(
                        Button::new("env-trigger", "Env")
                            .on_click(move |_, w, cx| toggle_env(w, cx)),
                    )
                    .on_dismiss(upd(&view, |v| {
                        v.env_open = false;
                        v.dismissed += 1;
                    }))
                    .child(
                        EnvPopover::new("envs", &envs)
                            .active(self.active_env)
                            .on_select(move |i, _, cx| {
                                select_view
                                    .update(cx, |v, cx| {
                                        v.active_env = i;
                                        v.env_open = false;
                                        cx.notify();
                                    })
                                    .ok();
                            }),
                    ),
            )
            .child(
                div()
                    .id("qty")
                    .w(px(80.))
                    .h(px(20.))
                    .tooltip(
                        Tooltip::new("quantity")
                            .part("integer")
                            .part("required")
                            .part("min: 1")
                            .mono()
                            .build(),
                    )
                    .test_support(),
            )
    }
}

fn open(cx: &mut TestAppContext) -> (Entity<Harness>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        Palette::dark().apply(cx);
    });
    let (view, window) = cx.add_window_view(|_, _| Harness::default());
    window.update(|window, cx| window.render_frame(cx));
    (view, window)
}

#[gpui_kit::test]
fn confirm_popover_opens_and_confirms(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| {
        assert!(
            w.try_find("panel").is_none(),
            "closed popover renders no panel"
        );
        w.click("send", cx);
        w.render_frame(cx);
        assert!(w.find("panel").visible());
        w.click("confirm", cx);
        w.render_frame(cx);
        assert!(w.try_find("panel").is_none());
    });
    view.read_with(window, |v, _| {
        assert_eq!((v.confirmed, v.cancelled), (1, 0));
        assert!(!v.open);
    });
}

#[gpui_kit::test]
fn confirm_popover_cancels(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| {
        w.click("send", cx);
        w.click("cancel", cx);
    });
    view.read_with(window, |v, _| {
        assert_eq!((v.confirmed, v.cancelled), (0, 1));
        assert!(!v.open);
    });
}

#[gpui_kit::test]
fn escape_dismisses(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| {
        w.click("send", cx);
        w.render_frame(cx);
        w.press("escape", cx);
    });
    view.read_with(window, |v, _| {
        assert!(!v.open);
        assert_eq!(v.cancelled, 1);
    });
}

#[gpui_kit::test]
fn click_outside_dismisses_but_trigger_does_not(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| {
        w.click("env-trigger", cx);
        w.render_frame(cx);
        // Clicking inside the panel is not "outside".
        w.click(("env", 0usize), cx);
    });
    view.read_with(window, |v, _| {
        assert_eq!(v.active_env, 0);
        assert_eq!(v.dismissed, 0);
    });

    window.update(|w, cx| {
        w.click("env-trigger", cx);
        w.render_frame(cx);
        assert!(w.find(("env", 2usize)).visible());
        w.click("outside", cx);
    });
    view.read_with(window, |v, _| {
        assert!(!v.env_open);
        assert_eq!(v.dismissed, 1);
    });

    // Trigger toggles closed without the outside handler re-opening/double-counting.
    window.update(|w, cx| {
        w.click("env-trigger", cx);
        w.click("env-trigger", cx);
    });
    view.read_with(window, |v, _| {
        assert!(!v.env_open);
        assert_eq!(v.dismissed, 1);
    });
}

#[gpui_kit::test]
fn env_popover_selects(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| {
        w.click("env-trigger", cx);
        w.within("envs").click(("env", 2usize), cx);
    });
    view.read_with(window, |v, _| {
        assert_eq!(v.active_env, 2);
        assert!(!v.env_open);
    });
}

#[gpui_kit::test]
fn tooltip_shows_on_hover(cx: &mut TestAppContext) {
    let (_view, window) = open(cx);
    window.update(|w, cx| w.hover("qty", cx));
    window.executor().advance_clock(Duration::from_secs(2));
    window.run_until_parked();
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(
            w.try_find("tooltip").is_some(),
            "tooltip rendered after hover delay"
        );
    });
}
