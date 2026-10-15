use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, div, px,
};
use rivet::theme::Palette;
use rivet::ui::{Annotation, CodeText, CodeView, KeyValueTable, KvRow};

struct Harness {
    rows: Vec<KvRow>,
    toggled: Vec<usize>,
    adds: usize,
    code: CodeText,
}

impl Harness {
    fn new() -> Self {
        let json: String = std::iter::once("{".to_string())
            .chain((0..10_000).map(|i| format!("  \"k{i}\": {i},")))
            .chain(std::iter::once("}".to_string()))
            .collect::<Vec<_>>()
            .join("\n");
        Self {
            rows: vec![
                KvRow::new("page", "1"),
                KvRow::new("token", "{{apiKey}}")
                    .description("auth")
                    .enabled(false),
            ],
            toggled: Vec::new(),
            adds: 0,
            code: CodeText::new(json),
        }
    }
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (on_toggle, on_add) = (cx.entity().downgrade(), cx.entity().downgrade());
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                KeyValueTable::new("params", self.rows.clone())
                    .on_toggle(move |ix, _, cx| {
                        let _ = on_toggle.update(cx, |v, cx| {
                            v.toggled.push(ix);
                            v.rows[ix].enabled = !v.rows[ix].enabled;
                            cx.notify();
                        });
                    })
                    .on_add(move |_, cx| {
                        let _ = on_add.update(cx, |v, cx| {
                            v.adds += 1;
                            v.rows.push(KvRow::new("", ""));
                            cx.notify();
                        });
                    }),
            )
            .child(
                div().h(px(200.)).child(
                    CodeView::new("body", self.code.clone())
                        .annotate(2, Annotation::Warning("does not match schema".into()))
                        .highlight_line(3),
                ),
            )
    }
}

fn open(cx: &mut TestAppContext) -> (Entity<Harness>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        Palette::dark().apply(cx);
    });
    let (view, window) = cx.add_window_view(|_, _| Harness::new());
    window.update(|window, cx| window.render_frame(cx));
    (view, window)
}

#[gpui_kit::test]
fn toggle_fires_with_row_index(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| {
        w.within("params").click(("toggle", 1usize), cx);
        w.within("params").click(("toggle", 0usize), cx);
    });
    view.read_with(window, |v, _| {
        assert_eq!(v.toggled, vec![1, 0]);
        assert!(v.rows[1].enabled);
        assert!(!v.rows[0].enabled);
    });
}

#[gpui_kit::test]
fn add_row_fires_and_renders_new_row(cx: &mut TestAppContext) {
    let (view, window) = open(cx);
    window.update(|w, cx| {
        assert!(w.within("params").try_find(("row", 2usize)).is_none());
        w.within("params").click("add", cx);
        w.render_frame(cx);
        assert!(w.within("params").try_find(("row", 2usize)).is_some());
    });
    assert_eq!(view.read_with(window, |v, _| v.adds), 1);
}

#[gpui_kit::test]
fn code_view_renders_only_visible_lines(cx: &mut TestAppContext) {
    let (_view, window) = open(cx);
    window.update(|w, _| {
        let body = w.within("body");
        assert!(body.try_find(("line", 0usize)).is_some());
        assert!(
            body.try_find(("line", 2usize)).is_some(),
            "annotated line renders"
        );
        assert!(
            body.try_find(("line", 9_000usize)).is_none(),
            "off-screen lines are virtualised"
        );
    });
}
