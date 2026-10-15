use gpui_kit::Hsla;
use gpui_kit::TestAppContext;
use gpui_kit::component::theme::Theme;
use rivet::theme::Palette;

fn syntax_color(cx: &mut TestAppContext, capture: &str) -> Option<Hsla> {
    cx.update(|cx| {
        Theme::global(cx)
            .highlight_theme
            .style
            .syntax
            .style(capture)
            .and_then(|s| s.color)
    })
}

#[gpui_kit::test]
fn json_highlighting_uses_rivet_palette(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        Palette::dark().apply(cx);
    });
    let p = Palette::dark();
    let close = |a: Option<Hsla>, b: Hsla| {
        let a = gpui_kit::Rgba::from(a.expect("capture styled"));
        let b = gpui_kit::Rgba::from(b);
        (a.r - b.r).abs() < 0.01 && (a.g - b.g).abs() < 0.01 && (a.b - b.b).abs() < 0.01
    };
    // Captures used by tree-sitter-json's highlights.scm.
    assert!(close(syntax_color(cx, "property"), p.accent_hi), "keys");
    assert!(close(syntax_color(cx, "string"), p.online), "strings");
    assert!(close(syntax_color(cx, "number"), p.amber), "numbers");
    assert!(close(syntax_color(cx, "boolean"), p.purple), "booleans");
    assert!(
        close(syntax_color(cx, "constant.builtin"), p.purple),
        "null"
    );
    assert!(
        close(syntax_color(cx, "punctuation"), p.text_dim),
        "punctuation"
    );

    // Switching theme re-applies the light palette's colours.
    cx.update(Palette::toggle);
    assert!(close(syntax_color(cx, "string"), Palette::light().online));
}
