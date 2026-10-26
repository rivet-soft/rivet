//! Cookie manager (like Postman's "Cookies" window): the cookies of every jar
//! by collection and domain, with delete / clear domain / add.

use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::Workspace;
use crate::core::http::{CookieInfo, UNSAVED_COOKIE_SCOPE};
use crate::core::project_ops;
use crate::theme::{ActivePalette, MONO};
use crate::ui::{Button, IconButton, IconName, Pill};

/// "через 3 ч" / "сессия" / "истекла".
fn expiry_label(expires: Option<i64>, now: i64) -> String {
    let Some(at) = expires else { return "сессия".into() };
    let left = at - now;
    match left {
        l if l <= 0 => "истекла".into(),
        l if l < 3600 => format!("через {} мин", (l / 60).max(1)),
        l if l < 86_400 => format!("через {} ч", l / 3600),
        l => format!("через {} дн", l / 86_400),
    }
}

impl Workspace {
    /// Jar the active request uses (its collection's, or the shared one).
    fn active_cookie_scope(&self) -> String {
        self.active()
            .and_then(|r| project_ops::cookie_scope(&self.project, r))
            .unwrap_or_else(|| UNSAVED_COOKIE_SCOPE.to_string())
    }

    fn cookie_scope_label(&self, scope: &str) -> String {
        if scope == UNSAVED_COOKIE_SCOPE {
            return "Запросы вне коллекций".into();
        }
        self.project
            .collections
            .iter()
            .find(|c| c.id == scope)
            .map_or_else(|| "Удалённая коллекция".into(), |c| c.name.clone())
    }

    pub fn open_cookies(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cookies_open = true;
        // Prefill the form with the active request's origin.
        let origin = self
            .active()
            .and_then(|r| crate::core::vars::resolve(r, self.request_env(r).as_ref()).ok())
            .map(|r| {
                let rest = r.url.split_once("://").map(|(s, rest)| (s.to_string(), rest.to_string()));
                rest.map_or(r.url.clone(), |(s, rest)| {
                    format!("{s}://{}", rest.split(['/', '?', '#']).next().unwrap_or(""))
                })
            })
            .unwrap_or_default();
        self.cookie_url.update(cx, |s, cx| s.set_value(origin, window, cx));
        cx.notify();
    }

    pub fn close_cookies(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cookies_open = false;
        // The form's inputs leave the tree: give focus back to the workspace,
        // or shortcuts (⌘↵…) stop reaching it.
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// "Добавить": the form's Set-Cookie line for the form's URL, into the
    /// active request's jar.
    pub fn add_cookie_from_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.cookie_url.read(cx).value().to_string();
        let line = self.cookie_value.read(cx).value().to_string();
        let scope = self.active_cookie_scope();
        match self.http_sessions.add_cookie(&scope, &url, &line) {
            Ok(()) => {
                self.cookie_value.update(cx, |s, cx| s.set_value("", window, cx));
                self.notify_user("Cookie добавлен", cx);
            }
            Err(e) => self.notify_user(e, cx),
        }
        cx.notify();
    }

    pub(super) fn render_cookie_manager(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *cx.palette();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let cookies = self.http_sessions.cookies();
        let mut list = div().id("cookie-list").flex().flex_col().gap(px(10.)).flex_1().min_h_0().overflow_y_scroll();
        if cookies.is_empty() {
            list = list.child(
                div()
                    .py(px(24.))
                    .text_color(p.text_dim)
                    .child("Cookies пока нет: они появятся после ответов с Set-Cookie или добавьте вручную."),
            );
        }
        // Group: scope → domain → cookies (already sorted).
        let mut ix = 0usize;
        let mut i = 0;
        while i < cookies.len() {
            let scope = cookies[i].scope.clone();
            list = list.child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.text_dim)
                    .child(self.cookie_scope_label(&scope).to_uppercase()),
            );
            while i < cookies.len() && cookies[i].scope == scope {
                let domain = cookies[i].domain.clone();
                let group: Vec<&CookieInfo> = cookies[i..]
                    .iter()
                    .take_while(|c| c.scope == scope && c.domain == domain)
                    .collect();
                i += group.len();
                let clear_scope = scope.clone();
                let clear_domain = domain.clone();
                let mut block = div()
                    .flex()
                    .flex_col()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(p.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .px(px(10.))
                            .h(px(30.))
                            .border_b_1()
                            .border_color(p.border)
                            .child(div().font_family(MONO).text_size(px(12.)).child(domain.clone()))
                            .child(
                                Button::new(("cookie-clear-domain", ix), "Удалить все")
                                    .ghost()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.http_sessions.clear_domain(&clear_scope, &clear_domain);
                                        cx.notify();
                                    })),
                            ),
                    );
                for c in group {
                    let (s, d, path, name) = (c.scope.clone(), c.domain.clone(), c.path.clone(), c.name.clone());
                    block = block.child(
                        div()
                            .id(("cookie", ix))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(10.))
                            .h(px(28.))
                            .text_size(px(12.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .font_family(MONO)
                                    .child(format!("{}={}", c.name, c.value)),
                            )
                            .child(div().text_color(p.text_dim).font_family(MONO).child(c.path.clone()))
                            .child(div().w(px(84.)).text_color(p.text_dim).child(expiry_label(c.expires, now)))
                            .when(c.secure, |d| d.child(Pill::new("Secure")))
                            .when(c.http_only, |d| d.child(Pill::new("HttpOnly")))
                            .child(
                                IconButton::new(("cookie-delete", ix), IconName::X)
                                    .size(12.)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.http_sessions.remove_cookie(&s, &d, &path, &name);
                                        cx.notify();
                                    })),
                            )
                            .test_support(),
                    );
                    ix += 1;
                }
                list = list.child(block);
            }
        }

        let disabled = !self.project.settings.cookies_enabled;
        let form = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .pt(px(10.))
            .border_t_1()
            .border_color(p.border)
            .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).child(format!(
                "Добавить в «{}»",
                self.cookie_scope_label(&self.active_cookie_scope())
            )))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(div().w(px(220.)).child(Input::new(&self.cookie_url).id("cookie-url").h(px(30.))))
                    .child(div().flex_1().child(Input::new(&self.cookie_value).id("cookie-value").font_family(MONO).h(px(30.))))
                    .child(
                        Button::new("cookie-add", "Добавить")
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| this.add_cookie_from_form(window, cx))),
                    ),
            );

        deferred(
            div()
                .id("cookies-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(90.))
                .bg(gpui_kit::black().opacity(0.35))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_cookies(window, cx)))
                .child(
                    div()
                        .id("cookie-manager")
                        .w(px(760.))
                        .h(px(520.))
                        .p(px(18.))
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .rounded(px(12.))
                        .bg(p.surface)
                        .border_1()
                        .border_color(p.border)
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child("Cookies"))
                                .child(
                                    IconButton::new("cookies-close", IconName::X)
                                        .on_click(cx.listener(|this, _, window, cx| this.close_cookies(window, cx))),
                                ),
                        )
                        .when(disabled, |d| {
                            d.child(div().text_color(p.warn).text_size(px(12.)).child(
                                "Сохранение cookies выключено в настройках: запросы их не отправляют.",
                            ))
                        })
                        .child(list)
                        .child(form)
                        .test_support(),
                ),
        )
        .with_priority(2)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::expiry_label;

    #[test]
    fn expiry_labels() {
        assert_eq!(expiry_label(None, 100), "сессия");
        assert_eq!(expiry_label(Some(50), 100), "истекла");
        assert_eq!(expiry_label(Some(100 + 30), 100), "через 1 мин");
        assert_eq!(expiry_label(Some(100 + 7200), 100), "через 2 ч");
        assert_eq!(expiry_label(Some(100 + 3 * 86_400), 100), "через 3 дн");
    }
}
