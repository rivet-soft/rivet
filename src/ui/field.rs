use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use std::ops::Range;

use gpui_kit::component::input::{
    InlineToken, InlineTokenContext, InlineTokenSpan, InputContent, InputState,
};

use super::{ClickHandler, IconName, Kbd, icon};
use crate::theme::{ActivePalette, MONO};

/// Read-only search trigger (titlebar ⌘K, sidebar filter). Opens a real input
/// or palette on click; typing is handled by whatever it opens.
#[derive(IntoElement)]
pub struct SearchField {
    id: ElementId,
    placeholder: SharedString,
    kbd: Option<SharedString>,
    on_click: Option<ClickHandler>,
}

impl SearchField {
    pub fn new(id: impl Into<ElementId>, placeholder: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            placeholder: placeholder.into(),
            kbd: None,
            on_click: None,
        }
    }

    pub fn kbd(mut self, kbd: impl Into<SharedString>) -> Self {
        self.kbd = Some(kbd.into());
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for SearchField {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();
        div()
            .id(self.id)
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(28.))
            .px(px(10.))
            .rounded(px(7.))
            .bg(p.surface_hi)
            .border_1()
            .border_color(p.border)
            .text_color(p.text_dim)
            .cursor_pointer()
            .hover(|s| s.border_color(p.accent))
            .when_some(self.on_click, |s, f| {
                s.on_click(move |e, w, cx| f(e, w, cx))
            })
            .child(icon(IconName::Search, 13., p.text_dim))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(self.placeholder),
            )
            .when_some(self.kbd, |s, k| s.child(Kbd::new(k)))
            .test_support()
    }
}

/// URL display with `{{variables}}` highlighted, as in the request line.
#[derive(IntoElement)]
pub struct UrlField {
    id: ElementId,
    url: SharedString,
}

impl UrlField {
    pub fn new(id: impl Into<ElementId>, url: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            url: url.into(),
        }
    }
}

/// Splits `text` into `(segment, is_variable)` runs around `{{…}}` markers.
pub fn split_vars(text: &str) -> Vec<(&str, bool)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let Some(len) = rest[start..].find("}}") else {
            break;
        };
        if start > 0 {
            out.push((&rest[..start], false));
        }
        out.push((&rest[start..start + len + 2], true));
        rest = &rest[start + len + 2..];
    }
    if !rest.is_empty() {
        out.push((rest, false));
    }
    out
}

/// Byte ranges of every `{{…}}` in `text`.
pub fn var_ranges(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut at = 0;
    for (seg, var) in split_vars(text) {
        if var {
            out.push(at..at + seg.len());
        }
        at += seg.len();
    }
    out
}

/// Variables in `text` that aren't yet covered by an inline token.
pub fn untokenized_vars(text: &str, tokens: &[InlineTokenSpan]) -> Vec<Range<usize>> {
    var_ranges(text)
        .into_iter()
        .filter(|r| !tokens.iter().any(|t| t.range() == *r))
        .collect()
}

fn var_token(text: &str) -> InlineToken {
    let name = text.trim_start_matches("{{").trim_end_matches("}}").trim();
    InlineToken::new(
        if name.is_empty() { "var" } else { name }.to_string(),
        text.to_string(),
    )
}

/// Text plus one atomic inline token per `{{variable}}`, ready for `set_value`.
pub fn var_content(text: &str) -> InputContent {
    var_ranges(text)
        .into_iter()
        .fold(InputContent::new(text.to_string()), |content, r| {
            let token = var_token(&text[r.clone()]);
            // Ranges come from split_vars, so they always match the text exactly.
            content.clone().with_token(r, token).unwrap_or(content)
        })
}

/// Turns any newly closed `{{…}}` in the input into a token. Call on
/// `InputEvent::Change`; a no-op once everything is tokenized.
pub fn tokenize_vars(state: &mut InputState, window: &mut Window, cx: &mut Context<InputState>) {
    let text = state.value();
    // Back to front: a replacement can't shift ranges still to be processed.
    for r in untokenized_vars(&text, state.tokens()).into_iter().rev() {
        let token = var_token(&text[r.clone()]);
        let _ = state.replace_range_with_token(r, token, window, cx);
    }
}

/// How a `{{variable}}` token looks inside a Kit `Input` (see `Input::token`).
pub fn var_token_element(ctx: &InlineTokenContext, cx: &App) -> impl IntoElement + use<> {
    let p = cx.palette();
    div()
        .flex()
        .items_center()
        .h(ctx.line_height())
        .px(px(3.))
        .rounded(px(4.))
        .font_family(MONO)
        .bg(if ctx.is_selected() {
            p.accent.opacity(0.35)
        } else {
            p.accent.opacity(0.14)
        })
        .text_color(p.accent_hi)
        .child(ctx.token().label().clone())
}

impl RenderOnce for UrlField {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();
        let runs = split_vars(&self.url)
            .into_iter()
            .map(|(seg, var)| {
                div()
                    .when(var, |s| {
                        s.px(px(3.))
                            .rounded(px(4.))
                            .bg(p.accent.opacity(0.14))
                            .text_color(p.accent_hi)
                    })
                    .child(SharedString::from(seg.to_string()))
            })
            .collect::<Vec<_>>();
        div()
            .id(self.id)
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .h_full()
            .px(px(10.))
            .overflow_hidden()
            .whitespace_nowrap()
            .font_family(MONO)
            .text_size(px(12.))
            .children(runs)
            .test_support()
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use super::{split_vars, untokenized_vars, var_content, var_ranges};

    #[test]
    fn var_ranges_and_content() {
        let text = "{{baseUrl}}/orders/{{ id }}";
        assert_eq!(var_ranges(text), vec![0..11, 19..27]);
        let content = var_content(text);
        assert_eq!(content.text().as_ref(), text);
        let ids: Vec<_> = content
            .tokens()
            .iter()
            .map(|t| t.token().id().to_string())
            .collect();
        assert_eq!(ids, ["baseUrl", "id"]);
        assert_eq!(
            untokenized_vars(text, content.tokens()),
            Vec::<Range<usize>>::new()
        );
        assert_eq!(untokenized_vars(text, &[]), vec![0..11, 19..27]);
        assert!(var_content("no vars").tokens().is_empty());
    }

    #[test]
    fn splits_variables() {
        assert_eq!(
            split_vars("{{baseUrl}}/orders/{{id}}"),
            vec![("{{baseUrl}}", true), ("/orders/", false), ("{{id}}", true)]
        );
        assert_eq!(split_vars("/plain"), vec![("/plain", false)]);
        assert_eq!(split_vars("/broken/{{x"), vec![("/broken/{{x", false)]);
    }
}
