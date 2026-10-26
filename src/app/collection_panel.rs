//! Collection management and collection-scoped variables.

use crate::core::export::ExportFormat;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::core::{Collection, KeyValue};
use crate::theme::ActivePalette;
use crate::ui::{Button, ConfirmPopover, IconButton, IconName, Placement};

#[derive(Clone)]
pub enum CollectionPanelEvent {
    Changed(Vec<Collection>),
    Reload(String),
    ClearCookies(String),
    /// Confirmed "Удалить коллекцию": drop it from the project (never its source file).
    Delete(String),
    /// Save the collection to a file in this format.
    Export(String, ExportFormat),
}
impl EventEmitter<CollectionPanelEvent> for CollectionPanel {}

struct VarRow {
    key: Entity<InputState>,
    value: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

pub struct CollectionPanel {
    collections: Vec<Collection>,
    selected: usize,
    name: Entity<InputState>,
    rows: Vec<VarRow>,
    /// The delete confirmation for the selected collection is open.
    pub confirm_delete: bool,
    _name_subscription: Subscription,
}

impl CollectionPanel {
    pub fn new(collections: Vec<Collection>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Название коллекции"));
        let name_subscription =
            cx.subscribe_in(&name, window, |this, state, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    if let Some(col) = this.collections.get_mut(this.selected) {
                        col.name = state.read(cx).value().to_string();
                        this.changed(cx);
                    }
                }
            });
        let mut this = Self {
            collections,
            selected: 0,
            name,
            rows: Vec::new(),
            confirm_delete: false,
            _name_subscription: name_subscription,
        };
        this.rebuild(window, cx);
        this
    }

    pub fn set_collections(
        &mut self,
        collections: Vec<Collection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.collections = collections;
        self.confirm_delete = false;
        self.selected = self.selected.min(self.collections.len().saturating_sub(1));
        self.rebuild(window, cx);
    }

    pub fn select(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.collections.len() {
            self.selected = index;
            self.confirm_delete = false;
            self.rebuild(window, cx);
        }
    }

    fn rebuild(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (name, vars) = self
            .collections
            .get(self.selected)
            .map(|c| (c.name.clone(), c.vars.clone()))
            .unwrap_or_default();
        self.name.update(cx, |s, cx| s.set_value(name, window, cx));
        self.rows = vars
            .iter()
            .enumerate()
            .map(|(index, var)| {
                let key = cx.new(|cx| {
                    let mut s = InputState::new(window, cx).placeholder("key");
                    s.set_value(var.key.clone(), window, cx);
                    s
                });
                let value = cx.new(|cx| {
                    let mut s = InputState::new(window, cx).placeholder("value");
                    s.set_value(var.value.clone(), window, cx);
                    s
                });
                let subscriptions = vec![
                    cx.subscribe_in(&key, window, move |this, state, ev: &InputEvent, _, cx| {
                        if let InputEvent::Change = ev {
                            if let Some(var) = this
                                .collections
                                .get_mut(this.selected)
                                .and_then(|c| c.vars.get_mut(index))
                            {
                                var.key = state.read(cx).value().to_string();
                                this.changed(cx);
                            }
                        }
                    }),
                    cx.subscribe_in(
                        &value,
                        window,
                        move |this, state, ev: &InputEvent, _, cx| {
                            if let InputEvent::Change = ev {
                                if let Some(var) = this
                                    .collections
                                    .get_mut(this.selected)
                                    .and_then(|c| c.vars.get_mut(index))
                                {
                                    var.value = state.read(cx).value().to_string();
                                    this.changed(cx);
                                }
                            }
                        },
                    ),
                ];
                VarRow {
                    key,
                    value,
                    _subscriptions: subscriptions,
                }
            })
            .collect();
        cx.notify();
    }

    fn changed(&self, cx: &mut Context<Self>) {
        cx.emit(CollectionPanelEvent::Changed(self.collections.clone()));
        cx.notify();
    }
    fn add_collection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.collections.push(Collection::new(format!(
            "Коллекция {}",
            self.collections.len() + 1
        )));
        self.selected = self.collections.len() - 1;
        self.rebuild(window, cx);
        self.changed(cx);
    }
    fn set_confirm_delete(&mut self, open: bool, cx: &mut Context<Self>) {
        self.confirm_delete = open && self.collections.get(self.selected).is_some();
        cx.notify();
    }
    fn confirm_delete_collection(&mut self, cx: &mut Context<Self>) {
        self.confirm_delete = false;
        if let Some(col) = self.collections.get(self.selected) {
            cx.emit(CollectionPanelEvent::Delete(col.id.clone()));
        }
        cx.notify();
    }
    fn add_var(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(col) = self.collections.get_mut(self.selected) {
            col.vars.push(KeyValue::new("", ""));
            self.rebuild(window, cx);
            self.changed(cx);
        }
    }
    fn delete_var(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(col) = self.collections.get_mut(self.selected) {
            if index < col.vars.len() {
                col.vars.remove(index);
                self.rebuild(window, cx);
                self.changed(cx);
            }
        }
    }
    fn toggle_var(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(var) = self
            .collections
            .get_mut(self.selected)
            .and_then(|c| c.vars.get_mut(index))
        {
            var.enabled = !var.enabled;
            self.changed(cx);
        }
    }

    fn toggle_cookies(&mut self, cx: &mut Context<Self>) {
        if let Some(collection) = self.collections.get_mut(self.selected) {
            collection.cookies_enabled = !collection.cookies_enabled;
            self.changed(cx);
        }
    }
}

impl CollectionPanel {
    fn delete_confirm(&self, col: &Collection, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity();
        delete_collection_confirm("collection-delete-confirm", col)
            .open(self.confirm_delete)
            .trigger(
                Button::new("collection-delete", "Удалить коллекцию")
                    .danger()
                    .on_click(cx.listener(|this, _, _, cx| {
                        let open = !this.confirm_delete;
                        this.set_confirm_delete(open, cx)
                    })),
            )
            .on_cancel({
                let view = view.clone();
                move |_, cx| {
                    view.update(cx, |this, cx| this.set_confirm_delete(false, cx));
                }
            })
            .on_confirm(move |_, cx| {
                view.update(cx, |this, cx| this.confirm_delete_collection(cx));
            })
            .into_any_element()
    }
}

/// The shared "Удалить коллекцию «X»?" confirmation (collection panel and sidebar).
pub(super) fn delete_collection_confirm(
    id: impl Into<ElementId>,
    col: &Collection,
) -> ConfirmPopover {
    let detail = col
        .source
        .as_ref()
        .map(|s| s.display().to_string())
        .unwrap_or_else(|| format!("{} запросов", col.requests.len()));
    ConfirmPopover::new(id, detail)
        .placement(Placement::BelowEnd)
        .title(format!(
            "Удалить коллекцию «{}»? Запросы коллекции будут удалены из Rivet. Исходный файл не изменится.",
            col.name
        ))
        .confirm_label("Удалить")
}

impl Render for CollectionPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *cx.palette();
        let selected = self.collections.get(self.selected);
        let delete = selected.map(|col| self.delete_confirm(col, cx));
        let items = self.collections.iter().enumerate().map(|(i, col)| {
            div()
                .id(("collection-list-item", i))
                .flex()
                .flex_col()
                .gap(px(3.))
                .px(px(12.))
                .py(px(9.))
                .cursor_pointer()
                .when(i == self.selected, |d| {
                    d.bg(p.surface_hi).text_color(p.accent)
                })
                .on_click(cx.listener(move |this, _, window, cx| this.select(i, window, cx)))
                .child(col.name.clone())
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(p.text_dim)
                        .child(format!("{} запросов", col.requests.len())),
                )
                .test_support()
        });
        let rows = selected
            .into_iter()
            .flat_map(|col| col.vars.iter().zip(&self.rows).enumerate())
            .map(|(i, (var, row))| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(38.))
                    .child(
                        Button::new(
                            ("collection-var-enabled", i),
                            if var.enabled { "✓" } else { "○" },
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_var(i, cx))),
                    )
                    .child(
                        div().w(px(210.)).child(
                            Input::new(&row.key)
                                .id(("collection-var-key", i))
                                .h(px(28.)),
                        ),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&row.value)
                                .id(("collection-var-value", i))
                                .h(px(28.)),
                        ),
                    )
                    .child(
                        IconButton::new(("collection-var-delete", i), IconName::X).on_click(
                            cx.listener(move |this, _, window, cx| this.delete_var(i, window, cx)),
                        ),
                    )
            });
        div().id("collection-panel").flex().flex_1().min_w_0()
            .child(div().w(px(260.)).flex_none().flex().flex_col().border_r_1().border_color(p.border).bg(p.surface)
                .child(div().flex().items_center().justify_between().px(px(12.)).py(px(12.))
                    .child("Коллекции")
                    .child(IconButton::new("collection-add", IconName::Plus)
                        .on_click(cx.listener(|this, _, window, cx| this.add_collection(window, cx)))))
                .child(div().id("collection-list-scroll").flex().flex_col().flex_1().overflow_y_scroll().children(items)))
            .child(div().id("collection-editor-scroll").flex().flex_col().flex_1().min_w_0().overflow_y_scroll().p(px(24.)).gap(px(18.))
                .when_some(selected.zip(delete), |d, (col, delete)| d
                    .child(div().flex().items_center().justify_between()
                        .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).child("Коллекция"))
                        .child(div().flex().gap(px(8.))
                            .when_some(col.source.as_ref(), |d, _| {
                                let id = col.id.clone();
                                d.child(Button::new("collection-reload", "Обновить из файла")
                                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(CollectionPanelEvent::Reload(id.clone())))))
                            })
                            .child(delete)))
                    .child(div().w(px(420.)).child(Input::new(&self.name).id("collection-name").h(px(30.))))
                    .child(div().text_color(p.text_dim).child(col.source.as_ref().map(|s| format!("Связанный файл: {}", s.display())).unwrap_or_else(|| "Ручная коллекция".into())))
                    .child(div().pt(px(12.)).border_t_1().border_color(p.border)
                        .text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child("Cookies"))
                    .child(div().text_color(p.text_dim).child("Запросы этой коллекции используют общий cookie jar до закрытия приложения, если сохранение включено в общих настройках."))
                    .child(div().flex().items_center().gap(px(8.))
                        .child(Button::new("collection-cookies-enabled", if col.cookies_enabled { "Включено" } else { "Выключено" })
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_cookies(cx))))
                        .child(Button::new("collection-cookies-clear", "Очистить cookies")
                            .on_click({ let id = col.id.clone(); cx.listener(move |_, _, _, cx| cx.emit(CollectionPanelEvent::ClearCookies(id.clone()))) })))
                    .child(div().pt(px(12.)).border_t_1().border_color(p.border)
                        .text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child("Экспорт"))
                    .child(div().text_color(p.text_dim).child("Rivet сохраняет всё без потерь. Postman и OpenAPI — для обмена с другими инструментами; то, что формат не поддерживает, будет перечислено после сохранения."))
                    .child(div().flex().flex_wrap().items_center().gap(px(8.))
                        .children(ExportFormat::ALL.into_iter().enumerate().map(|(i, format)| {
                            let id = col.id.clone();
                            Button::new(("collection-export", i), format.label())
                                .on_click(cx.listener(move |_, _, _, cx| cx.emit(CollectionPanelEvent::Export(id.clone(), format))))
                        })))
                    .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child("Переменные коллекции"))
                    .child(div().text_color(p.text_dim).child("Используйте {{var}} в URL, заголовках и теле. Значения коллекции имеют приоритет над окружением."))
                    .children(rows)
                    .child(Button::new("collection-var-add", "+ Переменная")
                        .on_click(cx.listener(|this, _, window, cx| this.add_var(window, cx)))))
                .when(selected.is_none(), |d| d.child("Создайте или импортируйте коллекцию")))
            .test_support()
    }
}
