use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{IconName, icon};
use crate::theme::{ActivePalette, MONO, Palette};

const LINE_H: f32 = 18.;
const FONT: f32 = 12.;
/// Approximate Menlo advance at 12px, used to size the gutter.
const CHAR_W: f32 = 7.3;
/// Longer lines (minified payloads) are cut for display so a single multi-MB line
/// never reaches the text shaper.
const MAX_LINE_BYTES: usize = 4096;

/// Text pre-split into lines. Build it once per response and keep it in the view
/// (clones are cheap); [`CodeView`] then only touches the visible lines per frame.
#[derive(Clone)]
pub struct CodeText {
    text: SharedString,
    line_starts: Rc<[usize]>,
    longest: usize,
}

impl CodeText {
    pub fn new(text: impl Into<SharedString>) -> Self {
        let text = text.into();
        let mut starts = vec![0];
        starts.extend(
            text.bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i + 1),
        );
        let line_starts: Rc<[usize]> = starts.into();
        let mut this = Self {
            text,
            line_starts,
            longest: 0,
        };
        this.longest = (0..this.line_count())
            .max_by_key(|&ix| this.line(ix).len())
            .unwrap_or(0);
        this
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Line `ix` without its trailing `\n` / `\r\n`.
    pub fn line(&self, ix: usize) -> &str {
        let start = self.line_starts[ix];
        let end = self
            .line_starts
            .get(ix + 1)
            .map_or(self.text.len(), |&next| next - 1);
        let line = &self.text[start..end];
        line.strip_suffix('\r').unwrap_or(line)
    }

    pub fn text(&self) -> &SharedString {
        &self.text
    }
}

impl From<&str> for CodeText {
    fn from(s: &str) -> Self {
        Self::new(SharedString::from(s.to_string()))
    }
}

impl From<String> for CodeText {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl From<SharedString> for CodeText {
    fn from(s: SharedString) -> Self {
        Self::new(s)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Key,
    String,
    Number,
    /// `true`, `false`, `null`.
    Literal,
    Punct,
    /// Anything that isn't valid JSON on its own (bare words, comments).
    Plain,
}

/// Per-line JSON tokenizer. Whitespace is not emitted; ranges are byte offsets
/// into `line`. A string followed by `:` is a [`TokenKind::Key`]. Strings left
/// open run to the end of the line, so broken input still colours sensibly.
pub fn tokenize_json_line(line: &str) -> Vec<(Range<usize>, TokenKind)> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let start = i;
        match c {
            b' ' | b'\t' | b'\r' => {
                i += 1;
                continue;
            }
            b'"' => {
                i += 1;
                while i < b.len() {
                    match b[i] {
                        b'\\' => i += 2,
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
                i = i.min(b.len());
                let mut j = i;
                while j < b.len() && matches!(b[j], b' ' | b'\t') {
                    j += 1;
                }
                let kind = if j < b.len() && b[j] == b':' {
                    TokenKind::Key
                } else {
                    TokenKind::String
                };
                out.push((start..i, kind));
            }
            b'{' | b'}' | b'[' | b']' | b',' | b':' => {
                i += 1;
                out.push((start..i, TokenKind::Punct));
            }
            b'-' | b'0'..=b'9' => {
                i += 1;
                while i < b.len() && matches!(b[i], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
                {
                    i += 1;
                }
                out.push((start..i, TokenKind::Number));
            }
            _ => {
                // Bare word (or any other run) up to the next delimiter.
                while i < b.len()
                    && !matches!(
                        b[i],
                        b' ' | b'\t' | b'\r' | b'"' | b'{' | b'}' | b'[' | b']' | b',' | b':'
                    )
                {
                    i += 1;
                }
                let kind = match &line[start..i] {
                    "true" | "false" | "null" => TokenKind::Literal,
                    _ => TokenKind::Plain,
                };
                out.push((start..i, kind));
            }
        }
    }
    out
}

fn token_color(kind: TokenKind, p: &Palette) -> Hsla {
    match kind {
        TokenKind::Key => p.accent_hi,
        TokenKind::String => p.online,
        TokenKind::Number => p.amber,
        TokenKind::Literal => p.purple,
        TokenKind::Punct => p.text_dim,
        TokenKind::Plain => p.text,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Annotation {
    /// Warn-tinted line with a trailing note, e.g. "response does not match schema".
    Warning(SharedString),
}

/// Read-only, virtualised code/JSON viewer with a line-number gutter.
///
/// Only visible lines are laid out and tokenized (`uniform_list`), lines never
/// wrap, and the list scrolls both ways. Each line is observable as `("line", ix)`.
#[derive(IntoElement)]
pub struct CodeView {
    id: ElementId,
    text: CodeText,
    json: bool,
    annotations: HashMap<usize, Annotation>,
    highlight: Option<usize>,
    scroll: Option<UniformListScrollHandle>,
}

impl CodeView {
    pub fn new(id: impl Into<ElementId>, text: impl Into<CodeText>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            json: true,
            annotations: HashMap::new(),
            highlight: None,
            scroll: None,
        }
    }

    /// Disable JSON colouring (plain text / unknown content types).
    pub fn plain(mut self) -> Self {
        self.json = false;
        self
    }

    pub fn annotate(mut self, line_ix: usize, annotation: Annotation) -> Self {
        self.annotations.insert(line_ix, annotation);
        self
    }

    pub fn highlight_line(mut self, ix: usize) -> Self {
        self.highlight = Some(ix);
        self
    }

    /// Lets the owning view keep scroll position / `scroll_to_item` the highlight.
    pub fn track_scroll(mut self, handle: &UniformListScrollHandle) -> Self {
        self.scroll = Some(handle.clone());
        self
    }
}

fn display_slice(line: &str) -> (&str, bool) {
    if line.len() <= MAX_LINE_BYTES {
        return (line, false);
    }
    let mut end = MAX_LINE_BYTES;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    (&line[..end], true)
}

struct LineCtx {
    text: CodeText,
    json: bool,
    annotations: HashMap<usize, Annotation>,
    highlight: Option<usize>,
    gutter_w: Pixels,
    p: Palette,
}

impl LineCtx {
    fn render_line(&self, ix: usize) -> impl IntoElement + use<> {
        let p = &self.p;
        let (shown, cut) = display_slice(self.text.line(ix));
        let highlights: Vec<(Range<usize>, HighlightStyle)> = if self.json {
            tokenize_json_line(shown)
                .into_iter()
                .map(|(r, k)| {
                    (
                        r,
                        HighlightStyle {
                            color: Some(token_color(k, p)),
                            ..Default::default()
                        },
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        let body =
            StyledText::new(SharedString::from(shown.to_string())).with_highlights(highlights);
        let warning = match self.annotations.get(&ix) {
            Some(Annotation::Warning(t)) => Some(t.clone()),
            None => None,
        };
        let highlighted = self.highlight == Some(ix);
        let (warn, accent, dim) = (p.warn, p.accent, p.text_dim);

        div()
            .id(("line", ix))
            .flex()
            .flex_none()
            .items_center()
            .min_w_full()
            .h(px(LINE_H))
            .pr(px(16.))
            .whitespace_nowrap()
            .when(highlighted, |s| s.bg(accent.opacity(0.12)))
            .when(warning.is_some(), |s| s.bg(warn.opacity(0.12)))
            .child(
                div()
                    .flex_none()
                    .w(self.gutter_w)
                    .pr(px(12.))
                    .text_right()
                    .text_color(if highlighted || warning.is_some() {
                        p.text
                    } else {
                        dim
                    })
                    .child(SharedString::from((ix + 1).to_string())),
            )
            .child(body)
            .when(cut, |s| {
                s.child(div().flex_none().pl(px(8.)).text_color(dim).child("…"))
            })
            .when_some(warning, |s, t| {
                s.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(4.))
                        .ml(px(16.))
                        .text_color(warn)
                        .child(icon(IconName::TriangleAlert, 11., warn))
                        .child(t),
                )
            })
            .test_support()
    }
}

impl RenderOnce for CodeView {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();
        let count = self.text.line_count();
        let digits = count.to_string().len().max(2) as f32;
        let longest = self.text.longest;
        let ctx = Rc::new(LineCtx {
            text: self.text,
            json: self.json,
            annotations: self.annotations,
            highlight: self.highlight,
            gutter_w: px(digits * CHAR_W + 24.),
            p,
        });

        let list_id = ElementId::NamedChild(std::sync::Arc::new(self.id.clone()), "lines".into());
        let list = uniform_list(list_id, count, move |range, _, _| {
            range.map(|ix| ctx.render_line(ix)).collect::<Vec<_>>()
        })
        .with_width_from_item(Some(longest))
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .size_full()
        .py(px(6.))
        .when_some(self.scroll, |s, h| s.track_scroll(&h));

        div()
            .id(self.id)
            .size_full()
            .overflow_hidden()
            .font_family(MONO)
            .text_size(px(FONT))
            .line_height(px(LINE_H))
            .text_color(p.text)
            .child(list)
            .test_support()
    }
}

#[cfg(test)]
mod tests {
    use super::{CodeText, TokenKind::*, tokenize_json_line};

    fn kinds(line: &str) -> Vec<(&str, super::TokenKind)> {
        tokenize_json_line(line)
            .into_iter()
            .map(|(r, k)| (&line[r], k))
            .collect()
    }

    #[test]
    fn key_value_pairs() {
        assert_eq!(
            kinds(r#"  "id": 42,"#),
            vec![(r#""id""#, Key), (":", Punct), ("42", Number), (",", Punct)]
        );
        assert_eq!(
            kinds(r#""name" : "Ann \"A\"""#),
            vec![(r#""name""#, Key), (":", Punct), (r#""Ann \"A\"""#, String)]
        );
    }

    #[test]
    fn literals_numbers_and_punct() {
        assert_eq!(
            kinds("[true, false, null, -1.5e+3]"),
            vec![
                ("[", Punct),
                ("true", Literal),
                (",", Punct),
                ("false", Literal),
                (",", Punct),
                ("null", Literal),
                (",", Punct),
                ("-1.5e+3", Number),
                ("]", Punct),
            ]
        );
        assert_eq!(kinds("{}"), vec![("{", Punct), ("}", Punct)]);
    }

    #[test]
    fn broken_input_is_tolerated() {
        assert_eq!(kinds(r#""open"#), vec![(r#""open"#, String)]);
        assert_eq!(kinds(r#""esc\"#), vec![(r#""esc\"#, String)]);
        assert_eq!(kinds("nope"), vec![("nope", Plain)]);
        assert_eq!(kinds("trueish"), vec![("trueish", Plain)]);
        assert!(tokenize_json_line("   ").is_empty());
        assert_eq!(
            kinds(r#""ключ": "знач""#),
            vec![(r#""ключ""#, Key), (":", Punct), (r#""знач""#, String)]
        );
    }

    #[test]
    fn code_text_lines() {
        let t = CodeText::from("a\r\nbbb\n\nc");
        assert_eq!(t.line_count(), 4);
        assert_eq!(
            (t.line(0), t.line(1), t.line(2), t.line(3)),
            ("a", "bbb", "", "c")
        );
        assert_eq!(t.longest, 1);
        assert_eq!(CodeText::from("").line_count(), 1);
    }
}
