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

const USAGE: &str = "Usage:
  rivet                         open the app
  rivet mcp [--project <file>]  MCP server over stdio (default project: the app's project.json)
  rivet --version";

/// Command-line modes that run without a window. Returns `None` for the GUI.
fn cli(args: &[String]) -> Option<i32> {
    match args.first().map(String::as_str) {
        Some("--version" | "-V" | "version") => {
            println!("Rivet {}", env!("RIVET_VERSION"));
            Some(0)
        }
        Some("--help" | "-h" | "help") => {
            println!("{USAGE}");
            Some(0)
        }
        Some("mcp") => Some(run_mcp(&args[1..])),
        _ => None,
    }
}

/// `rivet mcp`: stdout carries only protocol messages, diagnostics go to stderr.
fn run_mcp(args: &[String]) -> i32 {
    let mut project = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--project" | "-p" => match rest.next() {
                Some(path) => project = Some(std::path::PathBuf::from(path)),
                None => {
                    eprintln!("rivet mcp: --project needs a path\n{USAGE}");
                    return 2;
                }
            },
            other => match other.strip_prefix("--project=") {
                Some(path) => project = Some(std::path::PathBuf::from(path)),
                None => {
                    eprintln!("rivet mcp: unknown argument '{other}'\n{USAGE}");
                    return 2;
                }
            },
        }
    }
    let path = project.unwrap_or_else(|| {
        let path = store::default_path();
        store::migrate_legacy(&path, &store::legacy_path());
        path
    });
    eprintln!("rivet mcp {}: project {}", env!("RIVET_VERSION"), path.display());
    match rivet::mcp::serve_stdio(path) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("rivet mcp: {e}");
            1
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = cli(&args) {
        std::process::exit(code);
    }
    // Backups left by the previous self-update (`Rivet.app.old-*`, `*.old`).
    // A short delay lets the old process finish exiting so its files unlock.
    if !rivet::core::update::is_dev_build() {
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(3));
            rivet::core::update::cleanup_leftovers();
        });
    }
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
