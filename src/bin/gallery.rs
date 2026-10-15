//! UI kit gallery: `cargo run --bin gallery`.

use gpui_kit::component::TitleBar;
use gpui_kit::*;
use rivet::{gallery::Gallery, theme::Palette};

fn main() {
    gpui_kit::application()
        .with_assets(rivet::assets::RelayAssets)
        .run(|cx| {
            gpui_kit::init(cx);
            Palette::dark().apply(cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1200.), px(860.)), cx)),
                ..TitleBar::window_options()
            };
            gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| Gallery::new()))
                .expect("failed to open window");
            cx.activate(true);
        });
}
