//! Reusable modal for saving requests and naming collections, folders or requests.

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::rc::Rc;

use crate::core::Collection;
use crate::theme::ActivePalette;
use crate::ui::Button;

#[derive(Clone, Debug)]
pub enum DialogMode {
    SaveRequest,
    CreateFolder { collection_id: String },
    RenameFolder { collection_id: String, old: String },
    RenameCollection { collection_id: String },
    RenameRequest { request_id: String },
}

type ConfirmHandler =
    Rc<dyn Fn(DialogMode, String, Option<String>, Option<String>, &mut Window, &mut App)>;
type DismissHandler = Rc<dyn Fn(&mut Window, &mut App)>;

pub struct CollectionDialog {
    mode: DialogMode,
    name: Entity<InputState>,
    collections: Vec<Collection>,
    selected_collection: usize,
    selected_folder: Option<String>,
    on_confirm: ConfirmHandler,
    on_dismiss: DismissHandler,
}

impl CollectionDialog {
    pub fn new(
        mode: DialogMode,
        initial_name: String,
        collections: Vec<Collection>,
        preferred_collection: Option<String>,
        preferred_folder: Option<String>,
        on_confirm: ConfirmHandler,
        on_dismiss: DismissHandler,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let selected_collection = preferred_collection
            .and_then(|id| collections.iter().position(|c| c.id == id))
            .unwrap_or(0);
        let name = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("Название");
            state.set_value(initial_name, window, cx);
            state
        });
        Self {
            mode,
            name,
            collections,
            selected_collection,
            selected_folder: preferred_folder,
            on_confirm,
            on_dismiss,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.name.update(cx, |s, cx| s.focus(window, cx));
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        (self.on_confirm)(
            self.mode.clone(),
            name,
            self.collections
                .get(self.selected_collection)
                .map(|c| c.id.clone()),
            self.selected_folder.clone(),
            window,
            cx,
        );
    }
}

impl Render for CollectionDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *cx.palette();
        let viewport = window.viewport_size();
        let saving = matches!(self.mode, DialogMode::SaveRequest);
        let title = match self.mode {
            DialogMode::SaveRequest => "Сохранить запрос",
            DialogMode::CreateFolder { .. } => "Новая папка",
            DialogMode::RenameFolder { .. } => "Переименовать папку",
            DialogMode::RenameCollection { .. } => "Переименовать коллекцию",
            DialogMode::RenameRequest { .. } => "Переименовать запрос",
        };
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(div().text_color(p.text_dim).child("Название"))
            .child(
                Input::new(&self.name)
                    .id("collection-dialog-name")
                    .h(px(36.)),
            );
        if saving {
            body = body.child(div().pt(px(10.)).text_color(p.text_dim).child("Коллекция"));
            for (i, col) in self.collections.iter().enumerate() {
                body = body.child(
                    div()
                        .id(("dialog-collection", i))
                        .px(px(10.))
                        .py(px(7.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .when(i == self.selected_collection, |d| {
                            d.bg(p.surface_hi).text_color(p.accent)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected_collection = i;
                            this.selected_folder = None;
                            cx.notify();
                        }))
                        .child(col.name.clone())
                        .test_support(),
                );
            }
            body = body.child(div().pt(px(10.)).text_color(p.text_dim).child("Папка"));
            let root_selected = self.selected_folder.is_none();
            body = body.child(
                div()
                    .id("dialog-folder-root")
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(root_selected, |d| d.bg(p.surface_hi).text_color(p.accent))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.selected_folder = None;
                        cx.notify();
                    }))
                    .child("Без папки")
                    .test_support(),
            );
            if let Some(col) = self.collections.get(self.selected_collection) {
                let mut folders = col.folders.clone();
                for r in &col.requests {
                    if let Some(folder) = &r.folder {
                        if !folders.contains(folder) {
                            folders.push(folder.clone());
                        }
                    }
                }
                for (i, folder) in folders.into_iter().enumerate() {
                    let selected = self.selected_folder.as_deref() == Some(&folder);
                    let chosen_folder = folder.clone();
                    body = body.child(
                        div()
                            .id(("dialog-folder", i))
                            .px(px(10.))
                            .py(px(7.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .when(selected, |d| d.bg(p.surface_hi).text_color(p.accent))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected_folder = Some(chosen_folder.clone());
                                cx.notify();
                            }))
                            .child(format!("▸ {folder}"))
                            .test_support(),
                    );
                }
            }
        }
        let modal =
            div()
                .id("collection-dialog")
                .occlude()
                .w(px(420.))
                .max_w(viewport.width - px(32.))
                .max_h(viewport.height - px(64.))
                .flex()
                .flex_col()
                .bg(p.surface)
                .border_1()
                .border_color(p.border)
                .rounded(px(12.))
                .shadow(vec![BoxShadow {
                    color: black().opacity(0.4),
                    offset: point(px(0.), px(12.)),
                    blur_radius: px(36.),
                    spread_radius: px(0.),
                    inset: false,
                }])
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                    match e.keystroke.key.as_str() {
                        "escape" => {
                            cx.stop_propagation();
                            (this.on_dismiss)(window, cx);
                        }
                        "enter" => {
                            cx.stop_propagation();
                            this.confirm(window, cx);
                        }
                        _ => {}
                    }
                }))
                .child(
                    div()
                        .px(px(18.))
                        .py(px(14.))
                        .text_size(px(16.))
                        .child(title),
                )
                .child(
                    div()
                        .id("collection-dialog-body")
                        .px(px(18.))
                        .pb(px(12.))
                        .overflow_y_scroll()
                        .child(body),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .px(px(18.))
                        .py(px(12.))
                        .border_t_1()
                        .border_color(p.border)
                        .child(Button::new("dialog-cancel", "Отмена").on_click(
                            cx.listener(|this, _, window, cx| (this.on_dismiss)(window, cx)),
                        ))
                        .child(
                            Button::new(
                                "dialog-confirm",
                                if saving {
                                    "Сохранить"
                                } else {
                                    "Готово"
                                },
                            )
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx))),
                        ),
                )
                .test_support();
        deferred(
            anchored().position(point(px(0.), px(0.))).child(
                div()
                    .id("collection-dialog-backdrop")
                    .w(viewport.width)
                    .h(viewport.height)
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(black().opacity(0.48))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| (this.on_dismiss)(window, cx)),
                    )
                    .child(modal)
                    .test_support(),
            ),
        )
        .with_priority(10)
    }
}
