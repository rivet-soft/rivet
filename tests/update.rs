//! Self-update UI: rail button + indicator above Settings, popover, skip,
//! download → ready → install with fake hooks (nothing is installed or restarted).

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, ScrollDelta, TestAppContext, VisualTestContext, point, px};
use rivet::app::update_ui::DEV_BUILD_TIP;
use rivet::app::{self, UpdateHooks, UpdateState, Workspace};
use rivet::core::store;
use rivet::core::update::{Asset, ReleaseInfo, Version};
use rivet::model::RailMode;
use rivet::theme::Palette;

fn release() -> ReleaseInfo {
    ReleaseInfo {
        version: Version::new(99, 1, 0),
        tag: "v99.1.0".into(),
        name: "Rivet 99.1.0".into(),
        notes: "## Что нового\n- Автообновление".into(),
        html_url: "https://github.com/rivet-soft/rivet/releases/tag/v99.1.0".into(),
        published_at: "2026-10-12T09:30:00Z".into(),
        asset: Asset {
            name: "Rivet-99.1.0-macos-universal.zip".into(),
            browser_download_url: "https://example.invalid/pkg.zip".into(),
            size: 1_000,
        },
        checksums: None,
    }
}

fn open(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        Palette::dark().apply(cx);
        app::bind_keys(cx);
    });
    let (view, window) =
        cx.add_window_view(|window, cx| Workspace::new(store::seed_project(), None, window, cx));
    window.update(|w, cx| w.render_frame(cx));
    (view, window)
}

fn set_state(view: &Entity<Workspace>, window: &mut VisualTestContext, state: UpdateState) {
    view.update(window, |w, cx| w.set_update_state(state, cx));
    window.update(|w, cx| w.render_frame(cx));
}

fn label(window: &mut VisualTestContext, id: &'static str) -> String {
    window.update(|w, _| w.find(id).label().unwrap_or_default().to_string())
}

#[gpui_kit::test]
fn dev_build_shows_dimmed_button_without_popover(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    assert!(!view.read_with(window, |w, _| w.update_hooks.enabled), "tests run a dev build");
    window.update(|w, cx| {
        let button = w.find("rail-update").bounds();
        let settings = w.find("rail-settings").bounds();
        assert!(button.origin.y < settings.origin.y, "update button sits above Settings");
        w.click("rail-update", cx);
        w.render_frame(cx);
        assert!(w.try_find("update-popover").is_none());
        assert!(w.try_find("rail-update-dot").is_none());
    });
    assert_eq!(label(window, "rail-update"), DEV_BUILD_TIP);
    // Even an injected state shows no indicator while updates are disabled.
    set_state(&view, window, UpdateState::Available(release()));
    window.update(|w, _| assert!(w.try_find("rail-update-dot").is_none()));
}

#[gpui_kit::test]
fn available_update_shows_dot_popover_and_can_be_skipped(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    view.update(window, |w, cx| w.set_update_hooks(UpdateHooks::inert(), cx));
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("rail-update-dot").is_none());
    });

    set_state(&view, window, UpdateState::Available(release()));
    window.update(|w, cx| {
        assert!(w.try_find("rail-update-dot").is_some());
        assert!(w.try_find("update-popover").is_none());
        w.click("rail-update", cx);
        w.render_frame(cx);
        assert!(w.try_find("update-popover").is_some());
        assert!(w.try_find("update-download").is_some());
        assert!(w.try_find("update-release-page").is_some());
        // The popover opens to the right of the rail.
        let rail = w.find("rail-update").bounds();
        let popover = w.find("update-popover").bounds();
        assert!(popover.origin.x >= rail.origin.x + rail.size.width);
    });
    assert_eq!(label(window, "update-title"), "Rivet 99.1.0 доступен");
    assert!(label(window, "update-notes").contains("Автообновление"));
    assert_eq!(label(window, "rail-update"), "Доступна версия 99.1.0");

    window.update(|w, cx| {
        w.click("update-skip", cx);
        w.render_frame(cx);
        assert!(w.try_find("update-popover").is_none());
        assert!(w.try_find("rail-update-dot").is_none());
    });
    view.read_with(window, |w, cx| {
        assert_eq!(w.project.settings.skipped_version.as_deref(), Some("99.1.0"));
        // The Settings panel's copy agrees, so its next change keeps the skip.
        assert_eq!(
            w.settings_panel.read(cx).settings().skipped_version.as_deref(),
            Some("99.1.0")
        );
    });
    // A newer release than the skipped one shows the dot again.
    let mut newer = release();
    newer.version = Version::new(99, 2, 0);
    set_state(&view, window, UpdateState::Available(newer));
    window.update(|w, _| assert!(w.try_find("rail-update-dot").is_some()));
}

#[gpui_kit::test]
fn download_then_install_with_fake_hooks(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    let staged = PathBuf::from("/tmp/rivet-test-staged/Rivet.app");
    let installed: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
    let quit = Rc::new(Cell::new(false));
    let hooks = UpdateHooks {
        download: {
            let staged = staged.clone();
            Arc::new(move |release: &ReleaseInfo, _: &_, progress: rivet::core::http::Progress, _: &_| {
                assert_eq!(release.version, Version::new(99, 1, 0));
                progress(release.asset.size, release.asset.size);
                Ok(staged.clone())
            })
        },
        install: {
            let installed = installed.clone();
            Arc::new(move |path: &std::path::Path| {
                installed.lock().unwrap().push(path.to_path_buf());
                Ok(())
            })
        },
        on_installed: {
            let quit = quit.clone();
            Rc::new(move |_| quit.set(true))
        },
        ..UpdateHooks::inert()
    };
    view.update(window, |w, cx| {
        w.set_update_hooks(hooks, cx);
        w.update_popover_open = true;
    });
    set_state(&view, window, UpdateState::Available(release()));

    window.update(|w, cx| {
        w.click("update-download", cx);
        w.render_frame(cx);
        // Downloading: spinner indicator and progress bar.
        assert!(w.try_find("update-progress").is_some());
        assert!(w.try_find("rail-update-dot").is_some());
    });
    assert!(matches!(
        view.read_with(window, |w, _| w.update_state.clone()),
        UpdateState::Downloading { .. }
    ));
    window.run_until_parked();
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("update-progress").is_none());
        assert!(w.try_find("update-install").is_some());
        assert!(w.try_find("update-later").is_some());
        assert!(w.try_find("rail-update-dot").is_some());
    });
    assert_eq!(
        view.read_with(window, |w, _| w.update_state.clone()),
        UpdateState::Ready { release: release(), staged: staged.clone() }
    );

    // «Позже» closes the popover; the ready dot stays.
    window.update(|w, cx| {
        w.click("update-later", cx);
        w.render_frame(cx);
        assert!(w.try_find("update-popover").is_none());
        assert!(w.try_find("rail-update-dot").is_some());
        w.click("rail-update", cx);
        w.render_frame(cx);
        w.click("update-install", cx);
        w.render_frame(cx);
    });
    window.run_until_parked();
    assert_eq!(installed.lock().unwrap().as_slice(), &[staged]);
    assert!(quit.get(), "the app quits after a successful install");
}

#[gpui_kit::test]
fn failed_download_offers_retry(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    let attempts = Arc::new(Mutex::new(0));
    let hooks = UpdateHooks {
        download: {
            let attempts = attempts.clone();
            Arc::new(move |_: &ReleaseInfo, _: &_, _: rivet::core::http::Progress, _: &_| {
                *attempts.lock().unwrap() += 1;
                Err("Контрольная сумма не совпадает".into())
            })
        },
        ..UpdateHooks::inert()
    };
    view.update(window, |w, cx| {
        w.set_update_hooks(hooks, cx);
        w.update_popover_open = true;
    });
    set_state(&view, window, UpdateState::Available(release()));
    window.update(|w, cx| {
        w.click("update-download", cx);
        w.render_frame(cx);
    });
    window.run_until_parked();
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("update-retry").is_some());
        assert!(w.try_find("update-release-page").is_some());
        assert!(w.try_find("rail-update-dot").is_some());
        w.click("update-retry", cx);
        w.render_frame(cx);
    });
    window.run_until_parked();
    assert_eq!(*attempts.lock().unwrap(), 2);
    assert!(matches!(
        view.read_with(window, |w, _| w.update_state.clone()),
        UpdateState::Error { release: Some(_), .. }
    ));
}

#[gpui_kit::test]
fn settings_check_now_finds_update_and_opens_popover(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    let hooks = UpdateHooks { check: Arc::new(|_| Ok(Some(release()))), ..UpdateHooks::inert() };
    view.update(window, |w, cx| w.set_update_hooks(hooks, cx));
    window.update(|w, cx| {
        w.click("rail-settings", cx);
        w.render_frame(cx);
        w.scroll("settings-panel", ScrollDelta::Pixels(point(px(0.), px(-5000.))), cx);
        w.render_frame(cx);
        w.click("settings-auto-update", cx);
        w.render_frame(cx);
    });
    assert!(!view.read_with(window, |w, _| w.project.settings.auto_update));
    assert!(view.read_with(window, |w, _| w.rail == RailMode::Settings));

    window.update(|w, cx| {
        w.click("settings-check-updates", cx);
        w.render_frame(cx);
    });
    window.run_until_parked();
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("update-popover").is_some());
        assert!(w.try_find("rail-update-dot").is_some());
    });
    assert_eq!(label(window, "update-title"), "Rivet 99.1.0 доступен");
    view.read_with(window, |w, cx| {
        assert_eq!(w.settings_panel.read(cx).update_status.as_ref(), "Доступна версия 99.1.0");
    });

    // Up to date → «Проверить снова» in the popover.
    let hooks = UpdateHooks { check: Arc::new(|_| Ok(None)), ..UpdateHooks::inert() };
    view.update(window, |w, cx| {
        w.set_update_hooks(hooks, cx);
        w.set_update_state(UpdateState::Idle, cx);
    });
    window.update(|w, cx| {
        w.render_frame(cx);
        w.click("update-check-now", cx);
        w.render_frame(cx);
    });
    window.run_until_parked();
    window.update(|w, cx| w.render_frame(cx));
    assert!(matches!(view.read_with(window, |w, _| w.update_state.clone()), UpdateState::UpToDate(_)));
    assert!(label(window, "update-title").starts_with("У вас последняя версия"));
}
