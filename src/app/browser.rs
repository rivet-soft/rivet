//! "Browser" body mode: renders the response in a native webview (wry uses
//! WKWebView on macOS and WebView2 on Windows) with full CSS and JavaScript.
//! It is a native child view laid over a GPUI placeholder, so it must be hidden
//! whenever GPUI content needs to draw on top (palette, popovers) or the
//! placeholder isn't on screen.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::*;
use wry::dpi::{LogicalPosition, LogicalSize};

/// Builds the document the webview loads. HTML gets a `<base>` so relative
/// CSS/JS/images resolve against the request URL; anything else is shown as
/// escaped text.
pub fn browser_document(body: &str, is_html: bool, base_url: Option<&str>, dark: bool) -> String {
    if is_html {
        let Some(base) = base_url.filter(|b| !b.is_empty()) else {
            return body.to_string();
        };
        let tag = format!("<base href=\"{}\">", escape_html(base));
        let lower = body.to_ascii_lowercase();
        if lower.contains("<base ") {
            return body.to_string();
        }
        if let Some(ix) = lower.find("<head>") {
            let at = ix + "<head>".len();
            return format!("{}{}{}", &body[..at], tag, &body[at..]);
        }
        if let Some(ix) = lower.find("<html") {
            if let Some(end) = lower[ix..].find('>') {
                let at = ix + end + 1;
                return format!("{}<head>{}</head>{}", &body[..at], tag, &body[at..]);
            }
        }
        return format!("{tag}{body}");
    }
    let (bg, fg) = if dark {
        ("#0b0d12", "#e8ebf2")
    } else {
        ("#f6f7fb", "#121722")
    };
    format!(
        "<!doctype html><meta charset=utf-8><style>body{{margin:0;background:{bg};color:{fg}}}\
         pre{{margin:0;padding:14px;font:12px/1.5 Menlo,monospace;white-space:pre-wrap;word-break:break-word}}</style>\
         <pre>{}</pre>",
        escape_html(body)
    )
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[derive(Default)]
struct Inner {
    webview: Option<wry::WebView>,
    /// Document currently loaded, to avoid reloading every frame.
    loaded: Option<String>,
    /// Document to show.
    document: String,
    error: Option<String>,
    visible: bool,
    /// Times keyboard focus was handed back to the GPUI view (for tests/diagnostics).
    released: usize,
    /// Windows: popup window hosting the webview (declared after `webview`
    /// so the webview is dropped first).
    #[cfg(windows)]
    host: Option<super::browser_win::PopupHost>,
}

/// Owner of the native webview; cheap to clone (shared state).
#[derive(Clone, Default)]
pub struct BrowserPane(Rc<RefCell<Inner>>);

impl BrowserPane {
    pub fn set_document(&self, document: String) {
        self.0.borrow_mut().document = document;
    }

    pub fn error(&self) -> Option<String> {
        self.0.borrow().error.clone()
    }

    pub fn is_visible(&self) -> bool {
        self.0.borrow().visible
    }

    /// Hide the native view (it would otherwise float over GPUI content) and
    /// give the keyboard back to GPUI: a hidden native webview can still be the
    /// window's first responder and swallow every key press.
    pub fn hide(&self) {
        let mut inner = self.0.borrow_mut();
        if inner.visible {
            if let Some(w) = &inner.webview {
                let _ = w.set_visible(false);
                let _ = w.focus_parent();
            }
            #[cfg(windows)]
            if let Some(host) = &inner.host {
                host.hide();
                host.focus_owner();
            }
            inner.visible = false;
            inner.released += 1;
        }
    }

    /// Hand keyboard focus back to the GPUI view. Call on any GPUI mouse-down:
    /// clicks inside the page go to WebKit natively and never reach GPUI, so
    /// a GPUI click means the user left the page.
    pub fn release_keyboard(&self) {
        let mut inner = self.0.borrow_mut();
        if let Some(w) = &inner.webview {
            let _ = w.focus_parent();
        }
        #[cfg(windows)]
        if let Some(host) = &inner.host {
            host.focus_owner();
        }
        inner.released += 1;
    }

    /// Opens the current document in the system browser (a temp `.html`
    /// file; relative links still resolve through the injected `<base>`).
    pub fn open_external(&self, cx: &mut App) -> Result<(), String> {
        let document = self.0.borrow().document.clone();
        let path = std::env::temp_dir().join(format!("rivet-preview-{}.html", std::process::id()));
        std::fs::write(&path, document).map_err(|e| e.to_string())?;
        let url = url::Url::from_file_path(&path).map_err(|_| "invalid temp path".to_string())?;
        cx.open_url(url.as_str());
        Ok(())
    }

    pub fn keyboard_releases(&self) -> usize {
        self.0.borrow().released
    }

    /// Creates the webview on first use, loads the current document and
    /// places it over `bounds` (window coordinates, logical pixels).
    fn show_at(&self, bounds: Bounds<Pixels>, window: &mut Window) {
        #[cfg(windows)]
        {
            self.show_at_windows(bounds, window);
            return;
        }
        #[allow(unreachable_code)]
        let mut inner = self.0.borrow_mut();
        let rect = wry::Rect {
            position: LogicalPosition::new(
                f32::from(bounds.origin.x) as f64,
                f32::from(bounds.origin.y) as f64,
            )
            .into(),
            size: LogicalSize::new(
                f32::from(bounds.size.width) as f64,
                f32::from(bounds.size.height) as f64,
            )
            .into(),
        };
        if inner.webview.is_none() && inner.error.is_none() {
            let built = wry::WebViewBuilder::new()
                .with_bounds(rect)
                .with_html(inner.document.clone())
                .with_devtools(true)
                .build_as_child(&*window);
            match built {
                Ok(w) => {
                    // The native webview can grab keyboard focus when created/loaded;
                    // typing into the page starts only after clicking it.
                    let _ = w.focus_parent();
                    inner.webview = Some(w);
                    inner.loaded = Some(inner.document.clone());
                    inner.visible = true;
                }
                Err(e) => inner.error = Some(e.to_string()),
            }
            return;
        }
        let Some(w) = &inner.webview else { return };
        let _ = w.set_bounds(rect);
        if !inner.visible {
            let _ = w.set_visible(true);
        }
        let reload = inner.loaded.as_deref() != Some(inner.document.as_str());
        if reload {
            let _ = w.load_html(&inner.document);
            let _ = w.focus_parent();
        }
        inner.visible = true;
        if reload {
            inner.loaded = Some(inner.document.clone());
        }
    }

    /// Windows: the webview fills an owned popup placed over `bounds`.
    #[cfg(windows)]
    fn show_at_windows(&self, bounds: Bounds<Pixels>, window: &mut Window) {
        use wry::dpi::{PhysicalPosition, PhysicalSize};
        let mut inner = self.0.borrow_mut();
        let scale = window.scale_factor();
        let px = |v: Pixels| (f32::from(v) * scale).round() as i32;
        let (x, y, w, h) = (px(bounds.origin.x), px(bounds.origin.y), px(bounds.size.width), px(bounds.size.height));
        let rect = wry::Rect {
            position: PhysicalPosition::new(0, 0).into(),
            size: PhysicalSize::new(w.max(1) as u32, h.max(1) as u32).into(),
        };
        if inner.host.is_none() && inner.error.is_none() {
            match super::browser_win::owner_hwnd(&*window).and_then(super::browser_win::PopupHost::new) {
                Some(host) => inner.host = Some(host),
                None => {
                    inner.error = Some("не удалось создать окно для WebView2".into());
                    return;
                }
            }
        }
        let Some(host) = inner.host.as_ref() else { return };
        host.place(x, y, w, h);
        if inner.webview.is_none() && inner.error.is_none() {
            let built = wry::WebViewBuilder::new()
                .with_bounds(rect)
                .with_html(inner.document.clone())
                .with_devtools(true)
                .build_as_child(host);
            match built {
                Ok(webview) => {
                    host.focus_owner();
                    inner.webview = Some(webview);
                    inner.loaded = Some(inner.document.clone());
                    inner.visible = true;
                }
                Err(e) => inner.error = Some(e.to_string()),
            }
            return;
        }
        let Some(webview) = &inner.webview else { return };
        let _ = webview.set_bounds(rect);
        let _ = webview.set_visible(true);
        let reload = inner.loaded.as_deref() != Some(inner.document.as_str());
        if reload {
            let _ = webview.load_html(&inner.document);
            host.focus_owner();
            let document = inner.document.clone();
            inner.loaded = Some(document);
        }
        inner.visible = true;
    }

    /// Placeholder element whose painted bounds drive the native view.
    pub fn element(&self) -> impl IntoElement {
        let pane = self.clone();
        canvas(
            move |bounds, window, _| pane.show_at(bounds, window),
            |_, _, _, _| {},
        )
        .size_full()
    }
}

#[cfg(test)]
mod tests {
    use super::browser_document;

    #[test]
    fn injects_base_into_head() {
        let doc = browser_document(
            "<html><head><title>x</title></head><body></body></html>",
            true,
            Some("https://a.test/p/"),
            true,
        );
        assert!(doc.contains("<head><base href=\"https://a.test/p/\"><title>"));
    }

    #[test]
    fn adds_head_when_missing_and_keeps_existing_base() {
        let doc = browser_document(
            "<html lang=en><body>hi</body></html>",
            true,
            Some("https://a.test/"),
            true,
        );
        assert!(
            doc.starts_with("<html lang=en><head><base href=\"https://a.test/\"></head><body>")
        );
        let own = "<head><base href=\"/x/\"></head>";
        assert_eq!(
            browser_document(own, true, Some("https://a.test/"), true),
            own
        );
        assert!(
            browser_document("<p>frag</p>", true, Some("https://a.test/"), true)
                .starts_with("<base href=")
        );
    }

    #[test]
    fn non_html_is_escaped_text() {
        let doc = browser_document("{\"a\":\"<b>\"}", false, None, false);
        assert!(doc.contains("<pre>{&quot;a&quot;:&quot;&lt;b&gt;&quot;}</pre>"));
    }
}
