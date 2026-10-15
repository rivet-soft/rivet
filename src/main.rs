#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use gpui_kit::AppContext;
use rivet::app::{self, Workspace};
use rivet::core::store;
use rivet::theme::Palette;

#[cfg(target_os = "macos")]
fn install_app_icon() {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let bytes = include_bytes!("../assets/app-icon.png");
    let data = unsafe { NSData::dataWithBytes_length(bytes.as_ptr().cast(), bytes.len() as _) };
    if let Some(icon) = NSImage::initWithData(NSImage::alloc(), &data) {
        unsafe {
            NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&icon));
        }
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(rivet::assets::RelayAssets)
        .run(|cx| {
            gpui_kit::init(cx);
            Palette::dark().apply(cx);
            app::bind_keys(cx);
            cx.bind_keys([gpui_kit::KeyBinding::new("secondary-q", app::Quit, None)]);
            cx.on_action(|_: &app::Quit, cx| cx.quit());
            cx.set_menus(app::menu::app_menus());

            let path = store::default_path();
            // First launch after the Relay → Rivet rename: bring the old project over.
            store::migrate_legacy(&path, &store::legacy_path());
            let project = store::load_or_default(&path);
            gpui_kit::open_window(app::window_options(cx), cx, |window, cx| {
                cx.new(|cx| Workspace::new(project, Some(path), window, cx))
            })
            .expect("failed to open window");
            cx.on_window_closed(|cx, _| cx.quit()).detach();
            cx.activate(true);
            #[cfg(target_os = "macos")]
            install_app_icon();
        });
}
