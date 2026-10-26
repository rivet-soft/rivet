//! Relay palette, ported 1:1 from the design's `THEMES` tokens.

use gpui_kit::{App, Global, Hsla, rgb};

pub const MONO: &str = "Menlo";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Dark,
    Light,
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub mode: Mode,
    pub bg: Hsla,
    pub surface: Hsla,
    pub surface_hi: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub text_dim: Hsla,
    pub accent: Hsla,
    pub accent_hi: Hsla,
    pub danger: Hsla,
    pub online: Hsla,
    pub purple: Hsla,
    pub teal: Hsla,
    pub amber: Hsla,
    pub warn: Hsla,
}

impl Global for Palette {}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            mode: Mode::Dark,
            bg: c(0x0b0d12),
            surface: c(0x12151d),
            surface_hi: c(0x171b26),
            border: c(0x232838),
            text: c(0xe8ebf2),
            text_dim: c(0x8b93a7),
            accent: c(0x5b8cff),
            accent_hi: c(0x7aa2ff),
            danger: c(0xff6b6b),
            online: c(0x2fa98c),
            purple: c(0xa98bf5),
            teal: c(0x35c2b8),
            amber: c(0xe5a050),
            warn: c(0xe2c14e),
        }
    }

    pub fn light() -> Self {
        Self {
            mode: Mode::Light,
            bg: c(0xf6f7fb),
            surface: c(0xffffff),
            surface_hi: c(0xf0f2f8),
            border: c(0xe2e6f0),
            text: c(0x121722),
            text_dim: c(0x626b80),
            accent: c(0x3b6fe0),
            accent_hi: c(0x2f5fc9),
            danger: c(0xd64545),
            online: c(0x1f8a70),
            purple: c(0x7454d0),
            teal: c(0x11897f),
            amber: c(0xb06d16),
            warn: c(0x9a7a08),
        }
    }

    /// Installs the palette and mirrors it into gpui-component's theme, so Kit
    /// widgets we reuse (text inputs: caret, selection, placeholder) match.
    pub fn apply(self, cx: &mut App) {
        use gpui_kit::component::theme::{Theme, ThemeMode};
        cx.set_global(self);
        let mode = match self.mode {
            Mode::Dark => ThemeMode::Dark,
            Mode::Light => ThemeMode::Light,
        };
        Theme::change(mode, None, cx);
        Theme::update(cx, |t| {
            t.background = self.bg;
            t.foreground = self.text;
            t.muted_foreground = self.text_dim;
            t.border = self.border;
            t.input = self.border;
            t.ring = self.accent;
            t.primary = self.accent;
            t.caret = self.accent;
            t.selection = self.accent.opacity(0.3);
            t.popover = self.surface;
            t.popover_foreground = self.text;
            if let Some(h) = self.highlight_theme() {
                t.highlight_theme = std::sync::Arc::new(h);
            }
        });
        // Cached views (sidebar, editor, response) must repaint in the new palette.
        cx.refresh_windows();
    }

    /// Code editor colours (tree-sitter captures) matching `ui::CodeView`:
    /// keys accent, strings green, numbers amber, literals purple.
    fn highlight_theme(&self) -> Option<gpui_kit::component::highlighter::HighlightTheme> {
        let c = |h: Hsla| serde_json::json!({ "color": hex(h) });
        let value = serde_json::json!({
            "name": "Rivet",
            "appearance": if self.mode == Mode::Dark { "dark" } else { "light" },
            "style": {
                "editor.background": hex(self.bg),
                "editor.foreground": hex(self.text),
                "editor.active_line.background": hex(self.surface_hi),
                "editor.line_number": hex(self.text_dim.opacity(0.6)),
                "editor.active_line_number": hex(self.text_dim),
                "editor.invisible": hex(self.border),
                "editor.gutter.background": hex(self.bg),
                "syntax": {
                    "property": c(self.accent_hi),
                    "string": c(self.online),
                    "string.escape": c(self.teal),
                    "number": c(self.amber),
                    "boolean": c(self.purple),
                    "constant": c(self.purple),
                    "constant.builtin": c(self.purple),
                    "punctuation": c(self.text_dim),
                    "punctuation.bracket": c(self.text_dim),
                    "punctuation.delimiter": c(self.text_dim),
                    "comment": c(self.text_dim),
                    "keyword": c(self.purple),
                    "type": c(self.teal),
                    "function": c(self.accent),
                    "variable": c(self.text),
                    "operator": c(self.text_dim),
                }
            }
        });
        match serde_json::from_value(value) {
            Ok(theme) => Some(theme),
            Err(e) => {
                eprintln!("rivet: highlight theme rejected: {e}");
                None
            }
        }
    }

    pub fn toggle(cx: &mut App) {
        let next = match cx.global::<Palette>().mode {
            Mode::Dark => Palette::light(),
            Mode::Light => Palette::dark(),
        };
        next.apply(cx);
    }
}

/// `#rrggbbaa` for theme JSON.
fn hex(h: Hsla) -> String {
    let c = gpui_kit::Rgba::from(h);
    let b = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    format!("#{:02x}{:02x}{:02x}{:02x}", b(c.r), b(c.g), b(c.b), b(c.a))
}

pub trait ActivePalette {
    fn palette(&self) -> &Palette;
}

impl ActivePalette for App {
    fn palette(&self) -> &Palette {
        self.global::<Palette>()
    }
}
