use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, TestAppContext, VisualTestContext, Window, div, point, px,
};
use rivet::app::palette::{CommandPalette, PaletteEvent, PaletteItem, fuzzy_match};
use rivet::model::Method;
use rivet::theme::Palette;

struct Harness {
    palette: Entity<CommandPalette>,
    events: Vec<PaletteEvent>,
    _sub: Subscription,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.palette.clone())
    }
}

fn items() -> Vec<PaletteItem> {
    vec![
        PaletteItem::new("list-orders", "List orders", "/v2/orders").method(Method::Get),
        PaletteItem::new("create-order", "Create order", "/v2/orders").method(Method::Post),
        PaletteItem::new("get-product", "Get product", "/v2/products/{id}").method(Method::Get),
        PaletteItem::new("new-tab", "Новая вкладка", "Команды").kbd("⌘T"),
        PaletteItem::new("theme", "Переключить тему", "Команды").kbd("⌘⇧L"),
    ]
}

fn open(cx: &mut TestAppContext) -> (Entity<Harness>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        Palette::dark().apply(cx);
        rivet::app::palette::bind_keys(cx);
    });
    let (view, window) = cx.add_window_view(|window, cx| {
        let palette = cx.new(|cx| CommandPalette::new(items(), window, cx));
        let sub = cx.subscribe(&palette, |this: &mut Harness, _, ev: &PaletteEvent, _| {
            this.events.push(ev.clone());
        });
        palette.update(cx, |p, cx| p.focus(window, cx));
        Harness {
            palette,
            events: Vec::new(),
            _sub: sub,
        }
    });
    window.update(|w, cx| w.render_frame(cx));
    (view, window)
}

fn ids(view: &Entity<Harness>, window: &mut VisualTestContext) -> Vec<String> {
    view.read_with(window, |h, cx| {
        h.palette
            .read(cx)
            .visible_ids()
            .into_iter()
            .map(str::to_owned)
            .collect()
    })
}

fn events(view: &Entity<Harness>, window: &mut VisualTestContext) -> Vec<PaletteEvent> {
    view.read_with(window, |h, _| h.events.clone())
}

#[test]
fn fuzzy_prefix_beats_scattered() {
    let (prefix, ix) = fuzzy_match("ord", "Orders list").unwrap();
    assert_eq!(ix, vec![0, 1, 2]);
    let (scattered, _) = fuzzy_match("ord", "pOst Raw Data").unwrap();
    assert!(prefix > scattered, "{prefix} vs {scattered}");
    // Word start beats mid-word.
    let (word, _) = fuzzy_match("gp", "Get product").unwrap();
    let (mid, _) = fuzzy_match("gp", "upgrade plan").unwrap();
    assert!(word > mid);
    // Consecutive beats spread out.
    let (run, _) = fuzzy_match("order", "Create order").unwrap();
    let (spread, _) = fuzzy_match("order", "Open Read Dead Errors").unwrap();
    assert!(run > spread);
}

#[test]
fn fuzzy_cyrillic_and_case() {
    let (_, ix) = fuzzy_match("НОВ", "Новая вкладка").unwrap();
    assert_eq!(ix, vec![0, 1, 2]);
    let (_, ix) = fuzzy_match("нв", "Новая вкладка").unwrap();
    assert_eq!(ix, vec![0, 6]);
    assert!(fuzzy_match("ORD", "create order").is_some());
}

#[test]
fn fuzzy_no_match() {
    assert!(fuzzy_match("xyz", "List orders").is_none());
    assert!(fuzzy_match("sredro", "orders").is_none());
    assert!(fuzzy_match("toolongquery", "short").is_none());
}

#[gpui_kit::test]
fn empty_query_shows_all_in_order(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    assert_eq!(
        ids(&view, window),
        [
            "list-orders",
            "create-order",
            "get-product",
            "new-tab",
            "theme"
        ]
    );
    window.update(|w, _| {
        assert!(w.find("command-palette").visible());
        assert!(w.find(("palette-item", 4usize)).visible());
    });
}

#[gpui_kit::test]
fn typing_filters_results(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| w.input("prod", cx));
    assert_eq!(ids(&view, window), ["get-product"]);

    window.update(|w, cx| w.input("zzz", cx));
    assert!(ids(&view, window).is_empty());
    window.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.find("palette-empty").visible());
    });
}

#[gpui_kit::test]
fn typing_cyrillic_filters(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| w.input("тему", cx));
    assert_eq!(ids(&view, window), ["theme"]);
}

#[gpui_kit::test]
fn set_items_keeps_query(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| w.input("order", cx));
    let palette = view.read_with(window, |h, _| h.palette.clone());
    window.update(|w, cx| {
        palette.update(cx, |p, cx| {
            let mut more = items();
            more.push(PaletteItem::new(
                "delete-order",
                "Delete order",
                "/v2/orders/{id}",
            ));
            p.set_items(more, w, cx);
        })
    });
    assert!(ids(&view, window).contains(&"delete-order".to_owned()));
    assert!(!ids(&view, window).contains(&"theme".to_owned()));
    assert_eq!(
        palette.read_with(window, |p, _| p.query().to_owned()),
        "order"
    );
}

#[gpui_kit::test]
fn arrows_wrap_and_enter_picks(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| {
        w.press("down", cx);
        w.press("down", cx);
    });
    window.update(|w, cx| w.press("enter", cx));
    assert_eq!(
        events(&view, window),
        [PaletteEvent::Picked("get-product".into())]
    );

    // Up from the first row wraps to the last.
    window.update(|w, cx| {
        w.press("up", cx);
        w.press("up", cx);
        w.press("up", cx);
    });
    window.update(|w, cx| w.press("enter", cx));
    assert_eq!(
        events(&view, window).last(),
        Some(&PaletteEvent::Picked("theme".into()))
    );

    // ctrl-n wraps forward to the first row.
    window.update(|w, cx| w.press("ctrl-n", cx));
    window.update(|w, cx| w.press("enter", cx));
    assert_eq!(
        events(&view, window).last(),
        Some(&PaletteEvent::Picked("list-orders".into()))
    );

    window.update(|w, cx| w.press("ctrl-p", cx));
    let sel = view.read_with(window, |h, cx| {
        h.palette.read(cx).selected_id().map(str::to_owned)
    });
    assert_eq!(sel.as_deref(), Some("theme"));
}

#[gpui_kit::test]
fn enter_picks_filtered_selection(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| w.input("create", cx));
    window.update(|w, cx| w.press("enter", cx));
    assert_eq!(
        events(&view, window),
        [PaletteEvent::Picked("create-order".into())]
    );
}

#[gpui_kit::test]
fn escape_dismisses(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| w.press("escape", cx));
    assert_eq!(events(&view, window), [PaletteEvent::Dismissed]);
}

#[gpui_kit::test]
fn backdrop_click_dismisses_but_modal_click_does_not(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| w.click_at("command-palette", point(px(4.), px(4.)), cx));
    assert!(events(&view, window).is_empty());
    window.update(|w, cx| w.click_at("palette-backdrop", point(px(5.), px(5.)), cx));
    assert_eq!(events(&view, window), [PaletteEvent::Dismissed]);
}

#[gpui_kit::test]
fn hover_selects_and_click_picks(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| w.hover(("palette-item", 3usize), cx));
    let sel = view.read_with(window, |h, cx| h.palette.read(cx).selected_index());
    assert_eq!(sel, 3);
    window.update(|w, cx| w.click(("palette-item", 1usize), cx));
    assert_eq!(
        events(&view, window),
        [PaletteEvent::Picked("create-order".into())]
    );
}

#[gpui_kit::test]
fn many_items_scroll_selection_into_view(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    let palette = view.read_with(window, |h, _| h.palette.clone());
    window.update(|w, cx| {
        palette.update(cx, |p, cx| {
            let many = (0..50)
                .map(|i| PaletteItem::new(format!("r{i}"), format!("Request {i}"), "/x"))
                .collect();
            p.set_items(many, w, cx);
        })
    });
    window.update(|w, cx| {
        for _ in 0..20 {
            w.press("down", cx);
        }
        w.render_frame(cx);
        assert!(
            w.try_find(("palette-item", 20usize))
                .is_some_and(|e| e.visible())
        );
        assert!(
            w.try_find(("palette-item", 0usize))
                .is_none_or(|e| !e.visible())
        );
    });
}
