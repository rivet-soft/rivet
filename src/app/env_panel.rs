//! Environment editor (rail «Envs», PLAN.md app::env_panel): list of
//! environments on the left, name / colour / production flag and an editable
//! variables table for the selected one on the right.
//!
//! Element ids: `("env-item", ix)`, `"env-add"`, `"env-duplicate"`, `"env-delete"`,
//! `"env-activate"`, `"env-name"`, `("env-color", color_ix)`, `"env-production"`,
//! `("var-toggle", row)`, `("var-key", row)`, `("var-value", row)`, `("var-desc", row)`,
//! `("var-eye", row)`, `("var-delete", row)`, `"var-add"`.

use crate::ui::ScrollY as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::core::types::{EnvColor, Environment, KeyValue};
use crate::theme::{ActivePalette, MONO, Palette};
use crate::ui::{Button, IconButton, IconName, Pill, SectionLabel, StatusDot, icon};

pub const ENV_COLORS: [EnvColor; 5] = [
    EnvColor::Green,
    EnvColor::Red,
    EnvColor::Amber,
    EnvColor::Blue,
    EnvColor::Purple,
];

/// Palette colour for an environment colour tag.
pub fn env_color(c: EnvColor, p: &Palette) -> Hsla {
    match c {
        EnvColor::Green => p.online,
        EnvColor::Red => p.danger,
        EnvColor::Amber => p.amber,
        EnvColor::Blue => p.accent,
        EnvColor::Purple => p.purple,
    }
}

/// Keys whose values are masked by default (shared with the MCP server).
pub use crate::core::secrets::is_secret_key;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnvPanelEvent {
    /// Any edit to the environments (names, colours, vars, add/remove).
    Changed,
    /// "Сделать активным" was clicked for this environment index.
    Activated(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Key,
    Value,
    Description,
}

/// Editor widgets for one variable row of the selected environment.
struct VarRow {
    key: Entity<InputState>,
    value: Entity<InputState>,
    description: Entity<InputState>,
    /// Secret value shown in clear text.
    revealed: bool,
    _subscriptions: Vec<Subscription>,
}

pub struct EnvPanel {
    /// Scroll positions of this view's scroll areas (scrollbars).
    scrolls: crate::ui::ScrollHandles,
    envs: Vec<Environment>,
    active: usize,
    selected: usize,
    name_input: Entity<InputState>,
    rows: Vec<VarRow>,
    _name_subscription: Subscription,
}

impl EventEmitter<EnvPanelEvent> for EnvPanel {}

impl EnvPanel {
    pub fn new(
        envs: Vec<Environment>,
        active: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Название окружения"));
        let name_subscription = cx.subscribe_in(
            &name_input,
            window,
            |this, state, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    let name = state.read(cx).value().to_string();
                    if let Some(env) = this.envs.get_mut(this.selected) {
                        if env.name != name {
                            env.name = name;
                            cx.emit(EnvPanelEvent::Changed);
                            cx.notify();
                        }
                    }
                }
            },
        );
        let active = active.min(envs.len().saturating_sub(1));
        let mut this = Self {
            scrolls: Default::default(),
            envs,
            active,
            selected: active,
            name_input,
            rows: Vec::new(),
            _name_subscription: name_subscription,
        };
        this.rebuild(window, cx);
        this
    }

    pub fn envs(&self) -> &[Environment] {
        &self.envs
    }

    /// Index of the active environment (the one "Сделать активным" last chose).
    pub fn active(&self) -> usize {
        self.active
    }

    /// Index of the environment shown in the editor.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Replaces all environments (e.g. after loading a project). Emits nothing.
    pub fn set_envs(
        &mut self,
        envs: Vec<Environment>,
        active: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.envs = envs;
        self.active = active.min(self.envs.len().saturating_sub(1));
        self.selected = self.active;
        self.rebuild(window, cx);
    }

    /// Shows environment `ix` in the editor.
    pub fn select(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.envs.len() && ix != self.selected {
            self.selected = ix;
            self.rebuild(window, cx);
        }
    }

    pub fn activate(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.envs.len() {
            self.active = ix;
            cx.emit(EnvPanelEvent::Activated(ix));
            cx.notify();
        }
    }

    pub fn add_env(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut env =
            Environment::new(format!("Окружение {}", self.envs.len() + 1), EnvColor::Blue);
        env.vars.push(KeyValue::new("baseUrl", ""));
        self.envs.push(env);
        self.selected = self.envs.len() - 1;
        self.rebuild(window, cx);
        self.changed(cx);
    }

    pub fn duplicate_env(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(src) = self.envs.get(self.selected) else {
            return;
        };
        let mut env = src.clone();
        env.id = crate::core::types::new_id();
        env.name = format!("{} (копия)", src.name);
        self.envs.insert(self.selected + 1, env);
        if self.active > self.selected {
            self.active += 1;
        }
        self.selected += 1;
        self.rebuild(window, cx);
        self.changed(cx);
    }

    /// Removes the selected environment; the last remaining one can't be deleted.
    pub fn delete_env(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.envs.len() <= 1 {
            return;
        }
        let ix = self.selected;
        self.envs.remove(ix);
        let was_active = self.active == ix;
        if self.active > ix || self.active >= self.envs.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.selected = ix.min(self.envs.len() - 1);
        self.rebuild(window, cx);
        self.changed(cx);
        if was_active {
            cx.emit(EnvPanelEvent::Activated(self.active));
        }
    }

    pub fn add_var(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(env) = self.envs.get_mut(self.selected) else {
            return;
        };
        let kv = KeyValue::new("", "");
        env.vars.push(kv.clone());
        let row = Self::make_row(&kv, window, cx);
        row.key.update(cx, |s, cx| s.focus(window, cx));
        self.rows.push(row);
        self.changed(cx);
    }

    pub fn remove_var(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(env) = self.envs.get_mut(self.selected) else {
            return;
        };
        if row < env.vars.len() {
            env.vars.remove(row);
            self.rows.remove(row);
            self.changed(cx);
        }
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(EnvPanelEvent::Changed);
        cx.notify();
    }

    fn env_mut(&mut self) -> Option<&mut Environment> {
        self.envs.get_mut(self.selected)
    }

    /// Recreates the editor widgets for the selected environment, dropping the
    /// previous rows together with their subscriptions.
    fn rebuild(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (name, vars) = self
            .envs
            .get(self.selected)
            .map(|e| (e.name.clone(), e.vars.clone()))
            .unwrap_or_default();
        self.name_input
            .update(cx, |s, cx| s.set_value(name, window, cx));
        self.rows = vars
            .iter()
            .map(|kv| Self::make_row(kv, window, cx))
            .collect();
        cx.notify();
    }

    fn make_row(kv: &KeyValue, window: &mut Window, cx: &mut Context<Self>) -> VarRow {
        let secret = is_secret_key(&kv.key);
        let mk = |text: &str,
                  placeholder: &'static str,
                  masked: bool,
                  window: &mut Window,
                  cx: &mut Context<Self>| {
            let text = text.to_string();
            cx.new(|cx| {
                let mut s = InputState::new(window, cx)
                    .placeholder(placeholder)
                    .masked(masked);
                s.set_value(text, window, cx);
                s
            })
        };
        let key = mk(&kv.key, "key", false, window, cx);
        let value = mk(&kv.value, "value", secret, window, cx);
        let description = mk(&kv.description, "описание", false, window, cx);
        let subscriptions = [
            (&key, Field::Key),
            (&value, Field::Value),
            (&description, Field::Description),
        ]
        .into_iter()
        .map(|(state, field)| {
            cx.subscribe_in(
                state,
                window,
                move |this, state, ev: &InputEvent, window, cx| {
                    if let InputEvent::Change = ev {
                        this.on_cell_change(state, field, window, cx);
                    }
                },
            )
        })
        .collect();
        VarRow {
            key,
            value,
            description,
            revealed: false,
            _subscriptions: subscriptions,
        }
    }

    fn on_cell_change(
        &mut self,
        state: &Entity<InputState>,
        field: Field,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.rows.iter().position(|r| match field {
            Field::Key => r.key == *state,
            Field::Value => r.value == *state,
            Field::Description => r.description == *state,
        }) else {
            return;
        };
        let text = state.read(cx).value().to_string();
        let Some(kv) = self.env_mut().and_then(|e| e.vars.get_mut(ix)) else {
            return;
        };
        let slot = match field {
            Field::Key => &mut kv.key,
            Field::Value => &mut kv.value,
            Field::Description => &mut kv.description,
        };
        if *slot == text {
            return;
        }
        *slot = text;
        if field == Field::Key {
            // Typing a secret-looking key masks the value (unless revealed).
            let row = &self.rows[ix];
            let masked = is_secret_key(&self.envs[self.selected].vars[ix].key) && !row.revealed;
            row.value
                .update(cx, |s, cx| s.set_masked(masked, window, cx));
        }
        self.changed(cx);
    }

    fn toggle_var(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(kv) = self.env_mut().and_then(|e| e.vars.get_mut(ix)) {
            kv.enabled = !kv.enabled;
            self.changed(cx);
        }
    }

    fn toggle_reveal(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get_mut(ix) {
            row.revealed = !row.revealed;
            let masked = !row.revealed;
            row.value
                .update(cx, |s, cx| s.set_masked(masked, window, cx));
            cx.notify();
        }
    }

    fn set_color(&mut self, color: EnvColor, cx: &mut Context<Self>) {
        if let Some(env) = self.env_mut().filter(|e| e.color != color) {
            env.color = color;
            self.changed(cx);
        }
    }

    fn toggle_production(&mut self, cx: &mut Context<Self>) {
        if let Some(env) = self.env_mut() {
            env.production = !env.production;
            self.changed(cx);
        }
    }

    fn render_list(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.envs.iter().enumerate().map(|(ix, env)| {
            let selected = ix == self.selected;
            div()
                .id(("env-item", ix))
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(30.))
                .mx(px(6.))
                .px(px(10.))
                .rounded(px(6.))
                .cursor_pointer()
                .when(selected, |s| s.bg(p.surface_hi))
                .hover(|s| s.bg(p.surface_hi))
                .on_click(cx.listener(move |this, _, window, cx| this.select(ix, window, cx)))
                .child(StatusDot::new(env_color(env.color, p)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(12.5))
                        .text_color(if selected { p.text } else { p.text_dim })
                        .child(if env.name.is_empty() {
                            "Без названия".to_string()
                        } else {
                            env.name.clone()
                        }),
                )
                .when(env.production, |s| {
                    s.child(Pill::new("PROD").color(p.danger))
                })
                .when(ix == self.active, |s| {
                    s.child(icon(IconName::Check, 13., p.online))
                })
                .test_support()
        });

        let can_delete = self.envs.len() > 1;
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(240.))
            .h_full()
            .bg(p.surface)
            .border_r_1()
            .border_color(p.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pr(px(8.))
                    .child(SectionLabel::new("ENVIRONMENTS").caps())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(IconButton::new("env-add", IconName::Plus).on_click(
                                cx.listener(|this, _, window, cx| this.add_env(window, cx)),
                            ))
                            .child(IconButton::new("env-duplicate", IconName::Copy).on_click(
                                cx.listener(|this, _, window, cx| this.duplicate_env(window, cx)),
                            ))
                            .when(can_delete, |s| {
                                s.child(
                                    IconButton::new("env-delete", IconName::X)
                                        .color(p.danger)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.delete_env(window, cx)
                                        })),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .id("env-list")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .py(px(4.))
                    .scroll_y(&self.scrolls.get("env-list"))
                    .children(items),
            )
    }

    fn render_editor(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(env) = self.envs.get(self.selected) else {
            return div().flex_1().into_any_element();
        };
        let is_active = self.selected == self.active;

        let label = |t: &'static str| {
            div()
                .w(px(110.))
                .flex_none()
                .text_size(px(12.))
                .text_color(p.text_dim)
                .child(t)
        };

        let colors = ENV_COLORS.iter().enumerate().map(|(ci, &c)| {
            let on = env.color == c;
            let col = env_color(c, p);
            div()
                .id(("env-color", ci))
                .flex()
                .items_center()
                .justify_center()
                .size(px(20.))
                .rounded_full()
                .cursor_pointer()
                .border_2()
                .border_color(if on { col } else { transparent_black() })
                .child(StatusDot::new(col).size(12.))
                .on_click(cx.listener(move |this, _, _, cx| this.set_color(c, cx)))
                .test_support()
        });

        let prod_toggle = {
            let on = env.production;
            div()
                .id("env-production")
                .flex()
                .items_center()
                .gap(px(8.))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| this.toggle_production(cx)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .w(px(30.))
                        .h(px(17.))
                        .p(px(2.))
                        .rounded_full()
                        .bg(if on { p.danger } else { p.border })
                        .when(on, |s| s.justify_end())
                        .child(div().size(px(13.)).rounded_full().bg(gpui_kit::white())),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(if on { p.danger } else { p.text_dim })
                        .child(if on {
                            "Production — опасные запросы требуют подтверждения"
                        } else {
                            "Не production"
                        }),
                )
                .test_support()
        };

        let header = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(StatusDot::new(env_color(env.color, p)).size(10.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.text)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(if env.name.is_empty() {
                        "Без названия".to_string()
                    } else {
                        env.name.clone()
                    }),
            )
            .child(if is_active {
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .text_size(px(12.))
                    .text_color(p.online)
                    .child(icon(IconName::CircleCheck, 13., p.online))
                    .child("Активное")
                    .into_any_element()
            } else {
                let ix = self.selected;
                Button::new("env-activate", "Сделать активным")
                    .primary()
                    .on_click(cx.listener(move |this, _, _, cx| this.activate(ix, cx)))
                    .into_any_element()
            });

        div()
            .id("env-editor")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .scroll_y(&self.scrolls.get("env-editor"))
            .p(px(20.))
            .gap(px(14.))
            .child(header)
            .child(
                div().flex().items_center().child(label("Название")).child(
                    div()
                        .w(px(320.))
                        .child(Input::new(&self.name_input).id("env-name").h(px(28.))),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(label("Цвет"))
                    .child(div().flex().gap(px(6.)).children(colors)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(label("Production"))
                    .child(prod_toggle),
            )
            .child(self.render_vars(env, p, cx))
            .into_any_element()
    }

    fn render_vars(
        &self,
        env: &Environment,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        const TOGGLE_W: f32 = 28.;
        const KEY_W: f32 = 190.;
        const DESC_W: f32 = 220.;
        const ACTIONS_W: f32 = 52.;

        let head = |t: &'static str| {
            div()
                .text_size(px(10.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(p.text_dim)
                .child(t)
        };
        let header = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(26.))
            .px(px(8.))
            .border_b_1()
            .border_color(p.border)
            .child(div().w(px(TOGGLE_W)).flex_none())
            .child(head("KEY").w(px(KEY_W)).flex_none())
            .child(head("VALUE").flex_1())
            .child(head("DESCRIPTION").w(px(DESC_W)).flex_none())
            .child(div().w(px(ACTIONS_W)).flex_none());

        let rows =
            self.rows
                .iter()
                .zip(&env.vars)
                .enumerate()
                .map(|(ix, (row, kv))| {
                    let on = kv.enabled;
                    let secret = is_secret_key(&kv.key);
                    let toggle = div()
                        .id(("var-toggle", ix))
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(14.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .when(on, |s| {
                            s.bg(p.accent)
                                .child(icon(IconName::Check, 10., gpui_kit::white()))
                        })
                        .when(!on, |s| {
                            s.border_1()
                                .border_color(p.border)
                                .hover(|s| s.border_color(p.accent))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_var(ix, cx)))
                        .test_support();
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .h(px(36.))
                        .px(px(8.))
                        .border_b_1()
                        .border_color(p.border)
                        .child(
                            div()
                                .w(px(TOGGLE_W))
                                .flex_none()
                                .flex()
                                .items_center()
                                .child(toggle),
                        )
                        .child(
                            div()
                                .w(px(KEY_W))
                                .flex_none()
                                .when(!on, |s| s.opacity(0.6))
                                .child(
                                    Input::new(&row.key)
                                        .id(("var-key", ix))
                                        .font_family(MONO)
                                        .h(px(28.)),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .when(!on, |s| s.opacity(0.6))
                                .child(
                                    Input::new(&row.value)
                                        .id(("var-value", ix))
                                        .font_family(MONO)
                                        .h(px(28.)),
                                ),
                        )
                        .child(
                            div().w(px(DESC_W)).flex_none().child(
                                Input::new(&row.description).id(("var-desc", ix)).h(px(28.)),
                            ),
                        )
                        .child(
                            div()
                                .w(px(ACTIONS_W))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_end()
                                .when(secret, |s| {
                                    s.child(
                                        IconButton::new(
                                            ("var-eye", ix),
                                            if row.revealed {
                                                IconName::EyeOff
                                            } else {
                                                IconName::Eye
                                            },
                                        )
                                        .size(13.)
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                this.toggle_reveal(ix, window, cx)
                                            }),
                                        ),
                                    )
                                })
                                .child(
                                    IconButton::new(("var-delete", ix), IconName::X)
                                        .size(13.)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.remove_var(ix, cx)
                                        })),
                                ),
                        )
                });

        let add = div()
            .id("var-add")
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(32.))
            .pl(px(8. + TOGGLE_W + 6.))
            .text_size(px(12.))
            .text_color(p.text_dim)
            .cursor_pointer()
            .hover(|s| s.bg(p.surface_hi).text_color(p.text))
            .child(icon(IconName::Plus, 12., p.text_dim))
            .child("Добавить переменную")
            .on_click(cx.listener(|this, _, window, cx| this.add_var(window, cx)))
            .test_support();

        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.text)
                    .child("Переменные")
                    .child(Pill::new(env.vars.len().to_string()).mono()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(p.border)
                    .bg(p.surface)
                    .child(header)
                    .children(rows)
                    .child(add),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(p.text_dim)
                    .child("Используйте {{key}} в URL, заголовках и теле запроса."),
            )
    }
}

impl Render for EnvPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *cx.palette();
        div()
            .id("env-panel")
            .flex()
            .size_full()
            .bg(p.bg)
            .text_color(p.text)
            .child(self.render_list(&p, cx))
            .child(self.render_editor(&p, cx))
    }
}
