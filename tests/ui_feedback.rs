use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    Context, ElementId, Entity, IntoElement, ParentElement, Render, Styled, TestAppContext,
    VisualTestContext, Window, div,
};
use rivet::theme::Palette;
use rivet::ui::{Button, EmptyState, ErrorState, LoadingState, ProgressBar, Spinner};

#[derive(Default)]
struct Harness {
    retries: usize,
    proxy: usize,
    custom: usize,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(Spinner::new("spinner-small").small())
            .child(Spinner::new("spinner-large").large())
            .child(ProgressBar::new("progress"))
            .child(
                EmptyState::new("empty", "Ответ появится здесь")
                    .hint("Нажмите {kbd}, чтобы отправить запрос")
                    .kbd("⌘↵"),
            )
            .child(
                ErrorState::new("error", "Could not connect to dev.shop.local")
                    .description("connect ECONNREFUSED 127.0.0.1:8080")
                    .primary_action("Повторить", cx.listener(|this, _, _, _| this.retries += 1))
                    .secondary_action(
                        "Настройки прокси",
                        cx.listener(|this, _, _, _| this.proxy += 1),
                    ),
            )
            .child(
                ErrorState::new("error-custom", "Too many actions")
                    .action(
                        Button::new("act-1", "One")
                            .on_click(cx.listener(|this, _, _, _| this.custom += 1)),
                    )
                    .action(Button::new("act-2", "Two"))
                    .action(Button::new("act-3", "Three")),
            )
            .child(LoadingState::new("loading", "dev.shop.local"))
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

fn child(parent: impl Into<ElementId>, name: &'static str) -> ElementId {
    ElementId::NamedChild(std::sync::Arc::new(parent.into()), name.into())
}

#[gpui_kit::test]
fn feedback_components_render(cx: &mut TestAppContext) {
    let (_view, window) = open(cx);
    window.update(|w, _| {
        for id in [
            "spinner-small",
            "spinner-large",
            "progress",
            "empty",
            "error",
            "error-custom",
            "loading",
        ] {
            assert!(w.try_find(id).is_some(), "{id} not rendered");
        }
        assert!(
            w.try_find(child("loading", "spinner")).is_some(),
            "loading spinner not rendered"
        );
    });
}

#[gpui_kit::test]
fn error_state_actions_fire_handlers(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| {
        w.click(child("error", "primary"), cx);
        w.click(child("error", "primary"), cx);
        w.click(child("error", "secondary"), cx);
    });
    view.read_with(window, |v, _| {
        assert_eq!(v.retries, 2);
        assert_eq!(v.proxy, 1);
    });
}

#[gpui_kit::test]
fn error_state_renders_at_most_two_custom_buttons(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| {
        assert!(w.try_find("act-2").is_some());
        assert!(
            w.try_find("act-3").is_none(),
            "third action must be dropped"
        );
        w.click("act-1", cx);
    });
    assert_eq!(view.read_with(window, |v, _| v.custom), 1);
}
