use gpui_kit::test::TestWindowExt;
use gpui_kit::{TestAppContext, white};
use rivet::smoke::Smoke;
use rivet::theme::{Mode, Palette};

#[gpui_kit::test]
fn click_and_theme_toggle(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        Palette::dark().apply(cx);
    });
    let (view, window) = cx.add_window_view(|_, _| Smoke { clicks: 0 });
    let _ = white();

    window.update(|window, cx| {
        window.render_frame(cx);
        window.click("ping", cx);
        window.click("ping", cx);
    });
    assert_eq!(view.read_with(window, |v, _| v.clicks), 2);

    window.update(|window, cx| {
        window.click("toggle-theme", cx);
    });
    assert_eq!(
        window.update(|_, cx| cx.global::<Palette>().mode),
        Mode::Light
    );
}
