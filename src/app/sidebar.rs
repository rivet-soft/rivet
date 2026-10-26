use gpui_kit::component::input::Input;
use crate::ui::ScrollY as _;
use crate::core::tree_ops::{self, Clip, ClipItem};
use gpui_kit::*;
use gpui_kit::prelude::FluentBuilder as _;

use super::collection_dialog::DialogMode;
use super::collection_panel::delete_collection_confirm;
use super::{SidebarMenu, Workspace};
use crate::model::RailMode;
use crate::theme::ActivePalette;
use crate::ui::{
    Button, IconButton, IconName, Popover, SavedRequestDrag, SavedRequestDragPreview, SectionLabel,
    TreeRow, icon, shortcut_hint,
};

/// Default width of the sidebar (collections and history share one width).
pub(super) const SIDEBAR_DEFAULT_WIDTH: f32 = 260.;
/// Drag limits for the sidebar width.
pub(super) const SIDEBAR_MIN_WIDTH: f32 = 200.;
pub(super) const SIDEBAR_MAX_WIDTH: f32 = 520.;

impl Workspace {
    fn toggle_sidebar_menu(&mut self, target: SidebarMenu, cx: &mut Context<Self>) {
        if self.collection_menu.as_ref() == Some(&target) {
            self.collection_menu = None;
        } else {
            self.collection_menu = Some(target);
        }
        cx.notify();
    }

    fn new_request_in(
        &mut self,
        collection_id: String,
        folder: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_tab(window, cx);
        if let Some(tab) = self.active_mut() {
            tab.request.collection_id = Some(collection_id);
            tab.request.folder = folder;
        }
        self.collection_menu = None;
        cx.notify();
    }

    fn sidebar_menu_action(
        &mut self,
        target: SidebarMenu,
        action: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.collection_menu = None;
        match (target, action) {
            (target, "copy" | "cut") => {
                let item = match target {
                    SidebarMenu::Request(request_id) => self
                        .project
                        .collections
                        .iter()
                        .find(|c| c.requests.iter().any(|r| r.id == request_id))
                        .map(|c| ClipItem::Request { collection_id: c.id.clone(), request_id }),
                    SidebarMenu::Folder(collection_id, path) => {
                        Some(ClipItem::Folder { collection_id, path })
                    }
                    SidebarMenu::Collection(_) => None,
                };
                if let Some(item) = item {
                    let cut = action == "cut";
                    self.sidebar_clip = Some(Clip { item, cut });
                    self.notify_user(
                        if cut {
                            "Вырезано: «Вставить» в меню папки или коллекции перенесёт"
                        } else {
                            "Скопировано: «Вставить» в меню папки или коллекции"
                        },
                        cx,
                    );
                }
            }
            (SidebarMenu::Collection(id), "paste") => self.paste_into(&id, None, window, cx),
            (SidebarMenu::Collection(id), "mock-start") => self.start_mock(&id, cx),
            (SidebarMenu::Collection(id), "mock-stop") => self.stop_mock(&id, cx),
            (SidebarMenu::Folder(id, folder), "paste") => {
                self.paste_into(&id, Some(&folder), window, cx)
            }
            (SidebarMenu::Request(id), "duplicate") => {
                let new = self
                    .project
                    .collections
                    .iter_mut()
                    .find(|c| c.requests.iter().any(|r| r.id == id))
                    .and_then(|c| {
                        let new_id = tree_ops::duplicate_request(c, &id)?;
                        c.requests.iter().find(|r| r.id == new_id).cloned()
                    });
                if let Some(request) = new {
                    self.sync_collection_panel(window, cx);
                    self.schedule_save(cx);
                    self.notify_user(format!("Создан «{}»", request.name), cx);
                    self.open_request(request, window, cx);
                }
            }
            (SidebarMenu::Folder(id, folder), "duplicate") => {
                let path = self
                    .project
                    .collections
                    .iter_mut()
                    .find(|c| c.id == id)
                    .and_then(|c| tree_ops::duplicate_folder(c, &folder));
                if let Some(path) = path {
                    self.sync_collection_panel(window, cx);
                    self.schedule_save(cx);
                    self.notify_user(format!("Создана папка «{path}»"), cx);
                }
            }
            (SidebarMenu::Collection(id), "duplicate") => {
                if let Some(copy) = tree_ops::duplicate_collection(&self.project, &id) {
                    let name = copy.name.clone();
                    let at = self
                        .project
                        .collections
                        .iter()
                        .position(|c| c.id == id)
                        .map_or(self.project.collections.len(), |i| i + 1);
                    if let Some(spec) = self.linked_specs.get(&id).cloned() {
                        self.linked_specs.insert(copy.id.clone(), spec);
                    }
                    self.project.collections.insert(at, copy);
                    self.sync_collection_panel(window, cx);
                    self.schedule_save(cx);
                    self.notify_user(format!("Создана коллекция «{name}»"), cx);
                }
            }
            (SidebarMenu::Collection(id), export) if export.starts_with("export-") => {
                if let Some(format) =
                    crate::core::export::ExportFormat::from_id(&export["export-".len()..])
                {
                    self.prompt_export_collection(&id, format, cx);
                }
            }
            (SidebarMenu::Collection(id), "new-request") => {
                self.new_request_in(id, None, window, cx)
            }
            (SidebarMenu::Collection(id), "new-folder") => self.show_collection_dialog(
                DialogMode::CreateFolder { collection_id: id },
                String::new(),
                None,
                None,
                window,
                cx,
            ),
            (SidebarMenu::Collection(id), "rename") => {
                let name = self
                    .project
                    .collections
                    .iter()
                    .find(|c| c.id == id)
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                self.show_collection_dialog(
                    DialogMode::RenameCollection { collection_id: id },
                    name,
                    None,
                    None,
                    window,
                    cx,
                );
            }
            (SidebarMenu::Collection(id), "settings") => {
                if let Some(index) = self.project.collections.iter().position(|c| c.id == id) {
                    self.set_rail(RailMode::Collections, window, cx);
                    self.collection_panel
                        .update(cx, |panel, cx| panel.select(index, window, cx));
                }
            }
            (SidebarMenu::Collection(id), "delete") => {
                self.collection_delete_confirm = Some(id);
            }
            (SidebarMenu::Folder(id, folder), "new-request") => {
                self.new_request_in(id, Some(folder), window, cx)
            }
            (SidebarMenu::Folder(id, folder), "new-folder") => self.show_collection_dialog(
                DialogMode::CreateFolder { collection_id: id },
                format!("{folder} / "),
                None,
                None,
                window,
                cx,
            ),
            (SidebarMenu::Folder(id, folder), "rename") => {
                let name = folder.clone();
                self.show_collection_dialog(
                    DialogMode::RenameFolder {
                        collection_id: id,
                        old: folder,
                    },
                    name,
                    None,
                    None,
                    window,
                    cx,
                );
            }
            (SidebarMenu::Folder(id, folder), "delete") => {
                if let Some(col) = self.project.collections.iter_mut().find(|c| c.id == id) {
                    col.folders
                        .retain(|f| f != &folder && !f.starts_with(&format!("{folder} / ")));
                    for request in &mut col.requests {
                        if request
                            .folder
                            .as_ref()
                            .is_some_and(|f| f == &folder || f.starts_with(&format!("{folder} / ")))
                        {
                            request.folder = None;
                        }
                    }
                    for tab in &mut self.tabs {
                        if tab.request.collection_id.as_deref() == Some(&id)
                            && tab.request.folder.as_ref().is_some_and(|f| {
                                f == &folder || f.starts_with(&format!("{folder} / "))
                            })
                        {
                            tab.request.folder = None;
                        }
                    }
                    self.sync_collection_panel(window, cx);
                    self.schedule_save(cx);
                }
            }
            (SidebarMenu::Request(id), "rename") => {
                let name = self
                    .project
                    .collections
                    .iter()
                    .flat_map(|c| &c.requests)
                    .find(|r| r.id == id)
                    .map(|r| r.name.clone())
                    .unwrap_or_default();
                self.show_collection_dialog(
                    DialogMode::RenameRequest { request_id: id },
                    name,
                    None,
                    None,
                    window,
                    cx,
                );
            }
            (SidebarMenu::Request(id), "delete") => {
                for col in &mut self.project.collections {
                    col.requests.retain(|r| r.id != id);
                }
                self.sync_collection_panel(window, cx);
                self.schedule_save(cx);
            }
            _ => {}
        }
        cx.notify();
    }

    /// "Вставить": pastes the sidebar clipboard into a collection root or
    /// folder. Moved requests keep their open tabs in sync.
    fn paste_into(
        &mut self,
        collection_id: &str,
        folder: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(clip) = self.sidebar_clip.clone() else { return };
        match tree_ops::paste(&mut self.project, &clip, collection_id, folder) {
            Ok(pasted) => {
                for (request_id, collection_id, folder) in &pasted.moved {
                    let stored = self
                        .project
                        .collections
                        .iter()
                        .flat_map(|c| &c.requests)
                        .find(|r| &r.id == request_id)
                        .map(|r| r.spec_ref.clone());
                    for tab in self.tabs.iter_mut().filter(|t| &t.request.id == request_id) {
                        tab.request.collection_id = Some(collection_id.clone());
                        tab.request.folder = folder.clone();
                        if let Some(spec_ref) = stored.clone() {
                            tab.request.spec_ref = spec_ref;
                        }
                    }
                }
                if clip.cut {
                    self.sidebar_clip = None;
                }
                self.sync_collection_panel(window, cx);
                self.schedule_save(cx);
                self.notify_user(pasted.message, cx);
            }
            Err(e) => self.notify_user(e, cx),
        }
    }

    fn sidebar_menu_popover(
        &self,
        target: SidebarMenu,
        id: String,
        collection_ix: Option<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.collection_menu.as_ref() == Some(&target);
        let paste = self.sidebar_clip.is_some();
        let mock_running = match &target {
            SidebarMenu::Collection(id) => {
                matches!(self.mocks.get(id), Some(super::MockState::Running(_) | super::MockState::Starting))
            }
            _ => false,
        };
        let rows: Vec<(&'static str, &'static str)> = match target {
            SidebarMenu::Collection(_) => {
                let mut rows = vec![
                    ("new-request", "Новый запрос"),
                    ("new-folder", "Новая папка"),
                    ("rename", "Переименовать"),
                    ("duplicate", "Дублировать коллекцию"),
                    if mock_running { ("mock-stop", "Остановить мок") } else { ("mock-start", "Запустить мок") },
                ];
                if paste {
                    rows.push(("paste", "Вставить"));
                }
                rows.extend([
                    ("settings", "Настройки коллекции"),
                    ("export-rivet", "Экспорт: Rivet (без потерь)"),
                    ("export-postman", "Экспорт: Postman v2.1"),
                    ("export-openapi", "Экспорт: OpenAPI 3"),
                    ("delete", "Удалить коллекцию"),
                ]);
                rows
            }
            SidebarMenu::Folder(_, _) => {
                let mut rows = vec![
                    ("new-request", "Новый запрос"),
                    ("new-folder", "Новая вложенная папка"),
                    ("rename", "Переименовать"),
                    ("duplicate", "Дублировать"),
                    ("copy", "Копировать"),
                    ("cut", "Вырезать"),
                ];
                if paste {
                    rows.push(("paste", "Вставить"));
                }
                rows.push(("delete", "Удалить папку"));
                rows
            }
            SidebarMenu::Request(_) => vec![
                ("rename", "Переименовать"),
                ("duplicate", "Дублировать"),
                ("copy", "Копировать"),
                ("cut", "Вырезать"),
                ("delete", "Удалить запрос"),
            ],
        };
        let trigger = div()
            .id(format!("sidebar-menu-trigger-{id}"))
            .w(px(22.))
            .h(px(22.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .cursor_pointer()
            .child("···")
            .on_click(cx.listener({
                let trigger_target = target.clone();
                move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_sidebar_menu(trigger_target.clone(), cx);
                }
            }))
            .test_support();
        // Rows are on screen for every request: build the menu only when open.
        if !open {
            return trigger.into_any_element();
        }
        let mut pop = Popover::new(format!("sidebar-popover-{id}"))
            .open(open)
            .width(190.)
            .on_dismiss({
                let view = cx.entity();
                move |_, cx| {
                    view.update(cx, |this, cx| {
                        this.collection_menu = None;
                        cx.notify();
                    });
                }
            })
            .trigger(trigger);

        for (action, label) in rows {
            let target = target.clone();
            let button_id: ElementId = match (&target, action, collection_ix) {
                (SidebarMenu::Collection(_), "delete", Some(ix)) => {
                    ("sidebar-collection-delete", ix).into()
                }
                _ => format!("sidebar-action-{id}-{action}").into(),
            };
            pop = pop.child(Button::new(button_id, label).ghost().on_click(cx.listener(
                move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.sidebar_menu_action(target.clone(), action, window, cx);
                },
            )));
        }
        pop.into_any_element()
    }
    /// "Удалить коллекцию" confirmation anchored at the collection row's menu trigger.
    fn collection_delete_popover(
        &self,
        col: &crate::core::Collection,
        ci: usize,
        trigger: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.collection_delete_confirm.as_deref() == Some(col.id.as_str());
        let view = cx.entity();
        let collection_id = col.id.clone();
        delete_collection_confirm(("sidebar-collection-delete-confirm", ci), col)
            .open(open)
            .trigger(trigger)
            .on_cancel({
                let view = view.clone();
                move |_, cx| {
                    view.update(cx, |this, cx| {
                        this.collection_delete_confirm = None;
                        cx.notify();
                    });
                }
            })
            .on_confirm(move |window, cx| {
                view.update(cx, |this, cx| {
                    this.delete_collection(&collection_id, window, cx)
                });
            })
            .into_any_element()
    }

    /// Everything the sidebar draws from. The sidebar is a cached view (a large
    /// collection tree costs a lot to lay out); it re-renders only when this
    /// changes, not on every keystroke in the request editor.
    pub(super) fn sidebar_key(&self, cx: &App) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.rail as u8).hash(&mut h);
        self.sidebar_width.to_bits().hash(&mut h);
        self.filter.hash(&mut h);
        self.collection_menu.hash(&mut h);
        self.sidebar_clip.hash(&mut h);
        let mut mocks: Vec<(&String, u8)> = self
            .mocks
            .iter()
            .map(|(id, m)| {
                let state = match m {
                    super::MockState::Starting => 0,
                    super::MockState::Running(_) => 1,
                    super::MockState::Failed(_) => 2,
                };
                (id, state)
            })
            .collect();
        mocks.sort();
        mocks.hash(&mut h);
        self.collection_delete_confirm.hash(&mut h);
        let mut collapsed: Vec<&String> = self.collapsed.iter().collect();
        collapsed.sort();
        collapsed.hash(&mut h);
        self.active().map(|r| &r.id).hash(&mut h);
        self.modified_request_ids().hash(&mut h);
        (cx.palette().mode as u8).hash(&mut h);
        for c in &self.project.collections {
            (&c.id, &c.name, &c.folders, c.source.is_some()).hash(&mut h);
            for r in &c.requests {
                (&r.id, &r.name, r.method as u8, &r.folder, &r.url).hash(&mut h);
                for e in &r.examples {
                    (&e.id, &e.name).hash(&mut h);
                }
            }
        }
        h.finish()
    }

    pub(super) fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.rail {
            RailMode::History => self.render_history_sidebar(cx),
            _ => self.render_collections(cx),
        }
    }

    fn sidebar_shell(&self, width: f32, cx: &App) -> Div {
        let p = cx.palette();
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(width))
            // The sidebar is a cached view laid out as a root: without a
            // definite height it grows to its content and the tree never scrolls.
            .h_full()
            .bg(p.surface)
    }

    fn render_history_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        self.sidebar_shell(self.sidebar_width, cx)
            .child(self.history_panel.clone())
            .into_any_element()
    }

    fn render_collections(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *cx.palette();
        let active_id = self.active().map(|r| r.id.clone());
        let modified = self.modified_request_ids();

        let mut tree = div()
            .id("collection-tree")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .scroll_y(&self.scrolls.get("collection-tree"))
            .pb(px(8.));

        let filtering = !self.filter.is_empty();
        let matches = |name: &str, path: &str| {
            !filtering
                || name.to_lowercase().contains(&self.filter)
                || path.to_lowercase().contains(&self.filter)
        };

        // Imported OpenAPI/Swagger documents are linked collections, alongside
        // Postman and manually created collections.
        tree = tree.child(SectionLabel::new("Мои коллекции"));
        for (ci, col) in self.project.collections.iter().enumerate() {
            let explicit_folders = col.folders.clone();
            let visible: Vec<_> = col
                .requests
                .iter()
                .enumerate()
                .filter(|(_, r)| {
                    matches(&r.name, &r.url)
                        || r.folder
                            .as_deref()
                            .is_some_and(|folder| matches(folder, ""))
                })
                .collect();
            if filtering
                && visible.is_empty()
                && !matches(&col.name, "")
                && !explicit_folders.iter().any(|f| matches(f, ""))
            {
                continue;
            }
            let key = format!("col:{}", col.id);
            let open = filtering || !self.collapsed.contains(&key);
            let collection_id = col.id.clone();
            let drop_collection = collection_id.clone();
            let menu_target = SidebarMenu::Collection(collection_id.clone());
            let right_target = menu_target.clone();
            tree = tree.child(
                div()
                    .id(("collection-drop", ci))
                    .group("collection-row")
                    .relative()
                    .drag_over::<SavedRequestDrag>(move |style, _, _, _| {
                        style.bg(p.accent.opacity(0.17))
                    })
                    .on_drop(
                        cx.listener(move |this, drag: &SavedRequestDrag, window, cx| {
                            this.move_saved_request(
                                &drag.request_id,
                                &drop_collection,
                                None,
                                window,
                                cx,
                            )
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _, _, cx| {
                            this.collection_menu = Some(right_target.clone());
                            cx.notify();
                        }),
                    )
                    .child(
                        TreeRow::folder(("collection", ci), col.name.clone(), open)
                            .count(visible.len())
                            .trailing_padding(40.)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.collapsed.remove(&key) {
                                    this.collapsed.insert(key.clone());
                                }
                                cx.notify();
                            })),
                    )
                    .when(
                        matches!(self.mocks.get(&col.id), Some(super::MockState::Running(_))),
                        |d| {
                            d.child(
                                div()
                                    .id(("collection-mock-badge", ci))
                                    .absolute()
                                    .right(px(36.))
                                    .top(px(5.))
                                    .px(px(5.))
                                    .rounded(px(4.))
                                    .bg(p.online.opacity(0.15))
                                    .text_size(px(9.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.online)
                                    .child("MOCK")
                                    .test_support(),
                            )
                        },
                    )
                    .child(div().absolute().right(px(8.)).top(px(2.)).child(
                        self.collection_delete_popover(
                            col,
                            ci,
                            self.sidebar_menu_popover(
                                menu_target,
                                format!("collection-{ci}"),
                                Some(ci),
                                cx,
                            ),
                            cx,
                        ),
                    ))
                    .test_support(),
            );
            if !open {
                continue;
            }
            if visible.is_empty() && explicit_folders.is_empty() {
                tree = tree.child(
                    div()
                        .pl(px(34.))
                        .py(px(4.))
                        .text_size(px(11.))
                        .text_color(p.text_dim)
                        .child(format!(
                            "Пусто — сохраните запрос через {}",
                            shortcut_hint("⌘S", "Ctrl+S")
                        )),
                );
            }
            let mut folders: Vec<(Option<String>, Vec<(usize, &crate::core::RequestSpec)>)> =
                Vec::new();
            for folder in explicit_folders {
                if !filtering || matches(&folder, "") {
                    folders.push((Some(folder), Vec::new()));
                }
            }
            for (ri, request) in visible.iter().copied() {
                let folder = request.folder.clone();
                if let Some((_, requests)) = folders.iter_mut().find(|(name, _)| *name == folder) {
                    requests.push((ri, request));
                } else {
                    folders.push((folder, vec![(ri, request)]));
                }
            }
            // Materialize ancestors so Postman paths and user-created nested folders
            // appear as a tree, including intermediate folders with no direct requests.
            let mut nested =
                Vec::<(Option<String>, Vec<(usize, &crate::core::RequestSpec)>)>::new();
            for (path, requests) in folders {
                if let Some(path) = path {
                    let mut prefix = String::new();
                    for part in path.split(" / ") {
                        if !prefix.is_empty() {
                            prefix.push_str(" / ");
                        }
                        prefix.push_str(part);
                        if !nested
                            .iter()
                            .any(|(name, _)| name.as_deref() == Some(&prefix))
                        {
                            nested.push((Some(prefix.clone()), Vec::new()));
                        }
                    }
                    nested
                        .iter_mut()
                        .find(|(name, _)| name.as_deref() == Some(&path))
                        .unwrap()
                        .1
                        .extend(requests);
                } else {
                    nested.push((None, requests));
                }
            }
            for (fi, (folder, requests)) in nested.into_iter().enumerate() {
                let mut request_depth = 1;
                if let Some(folder) = folder {
                    let mut ancestor = folder
                        .rsplit_once(" / ")
                        .map(|(parent, _)| parent.to_string());
                    let mut hidden = false;
                    while let Some(parent) = ancestor {
                        if !filtering
                            && self
                                .collapsed
                                .contains(&format!("col:{}:folder:{parent}", col.id))
                        {
                            hidden = true;
                            break;
                        }
                        ancestor = parent.rsplit_once(" / ").map(|(next, _)| next.to_string());
                    }
                    if hidden {
                        continue;
                    }
                    let folder_depth = folder.split(" / ").count();
                    request_depth = folder_depth + 1;
                    let folder_label = folder.rsplit(" / ").next().unwrap_or(&folder).to_string();
                    let descendant_prefix = format!("{folder} / ");
                    let count = visible
                        .iter()
                        .filter(|(_, r)| {
                            r.folder.as_deref().is_some_and(|path| {
                                path == folder || path.starts_with(&descendant_prefix)
                            })
                        })
                        .count();
                    let key = format!("col:{}:folder:{folder}", col.id);
                    let folder_open = filtering || !self.collapsed.contains(&key);
                    let folder_target = SidebarMenu::Folder(col.id.clone(), folder.clone());
                    let right_target = folder_target.clone();
                    let drop_collection = col.id.clone();
                    let drop_folder = folder.clone();
                    tree = tree.child(
                        div()
                            .id(("folder-drop", ci * 1000 + fi))
                            .relative()
                            .drag_over::<SavedRequestDrag>(move |style, _, _, _| {
                                style.bg(p.accent.opacity(0.17))
                            })
                            .on_drop(cx.listener(
                                move |this, drag: &SavedRequestDrag, window, cx| {
                                    this.move_saved_request(
                                        &drag.request_id,
                                        &drop_collection,
                                        Some(drop_folder.clone()),
                                        window,
                                        cx,
                                    )
                                },
                            ))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, _, _, cx| {
                                    this.collection_menu = Some(right_target.clone());
                                    cx.notify();
                                }),
                            )
                            .child(
                                TreeRow::folder(
                                    ("collection-folder", ci * 1000 + fi),
                                    folder_label,
                                    folder_open,
                                )
                                .depth(folder_depth)
                                .count(count)
                                .trailing_padding(40.)
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if !this.collapsed.remove(&key) {
                                            this.collapsed.insert(key.clone());
                                        }
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(div().absolute().right(px(8.)).top(px(2.)).child(
                                self.sidebar_menu_popover(
                                    folder_target,
                                    format!("folder-{ci}-{fi}"),
                                    None,
                                    cx,
                                ),
                            ))
                            .test_support(),
                    );
                    if !folder_open {
                        continue;
                    }
                }
                for (ri, r) in requests {
                    let selected = active_id.as_deref() == Some(r.id.as_str());
                    let drag = SavedRequestDrag {
                        request_id: r.id.clone(),
                        label: r.name.clone(),
                    };
                    let request_target = SidebarMenu::Request(r.id.clone());
                    let right_target = request_target.clone();
                    tree = tree.child(
                        div()
                            .id(("saved-drag", ci * 1000 + ri))
                            .group("saved-row")
                            .relative()
                            .on_drag(drag, |drag, _, _, cx| {
                                cx.new(|_| SavedRequestDragPreview {
                                    label: drag.label.clone(),
                                })
                            })
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, _, _, cx| {
                                    this.collection_menu = Some(right_target.clone());
                                    cx.notify();
                                }),
                            )
                            .child(if r.is_ws() {
                                super::ws_view::ws_tree_row(
                                    ("saved-ws", ci * 1000 + ri),
                                    r.name.clone(),
                                    &r.url,
                                    request_depth,
                                    selected,
                                    cx.listener(move |this, _, window, cx| {
                                        this.open_saved(ci, ri, window, cx)
                                    }),
                                    cx,
                                )
                                .test_support()
                                .into_any_element()
                            } else {
                                TreeRow::request(
                                    ("saved", ci * 1000 + ri),
                                    r.name.clone(),
                                    r.method,
                                )
                                .depth(request_depth)
                                .selected(selected)
                                .modified(modified.binary_search(&r.id).is_ok())
                                // Room for the absolutely positioned "···" menu button.
                                .trailing_padding(40.)
                                .on_click(cx.listener(
                                    move |this, _, window, cx| this.open_saved(ci, ri, window, cx),
                                ))
                                .into_any_element()
                            })
                            .child(div().absolute().right(px(8.)).top(px(2.)).child(
                                self.sidebar_menu_popover(
                                    request_target,
                                    format!("request-{ci}-{ri}"),
                                    None,
                                    cx,
                                ),
                            ))
                            .test_support(),
                    );
                    for (ei, example) in r.examples.iter().enumerate() {
                        tree = tree.child(
                            div()
                                .id(format!("example-{ci}-{ri}-{ei}"))
                                .group("example-row")
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .h(px(24.))
                                .mx(px(4.))
                                .pl(px(46. + 12. * request_depth as f32))
                                .pr(px(6.))
                                .rounded(px(5.))
                                .cursor_pointer()
                                .text_size(px(11.5))
                                .text_color(p.text_dim)
                                .hover(|s| s.bg(p.surface_hi))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_example(ci, ri, ei, window, cx)
                                }))
                                .child(icon(IconName::Bookmark, 11., p.text_dim))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .child(example.name.clone()),
                                )
                                .child(
                                    div()
                                        .invisible()
                                        .group_hover("example-row", |s| s.visible())
                                        .child(
                                            IconButton::new(
                                                format!("example-delete-{ci}-{ri}-{ei}"),
                                                IconName::X,
                                            )
                                            .size(11.)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.delete_example(ci, ri, ei, cx)
                                            })),
                                        ),
                                )
                                .test_support(),
                        );
                    }
                }
            }
        }

        self.sidebar_shell(self.sidebar_width, cx)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pr(px(8.))
                    .child(SectionLabel::new("COLLECTIONS").caps())
                    .child(
                        IconButton::new("sidebar-open", IconName::Plus)
                            .on_click(cx.listener(|this, _, window, cx| this.new_tab(window, cx))),
                    ),
            )
            .child(
                div().px(px(10.)).pb(px(6.)).child(
                    Input::new(&self.filter_input)
                        .id("sidebar-search")
                        .prefix(icon(IconName::Search, 13., p.text_dim))
                        .cleanable(true)
                        .h(px(28.)),
                ),
            )
            .child(tree)
            .into_any_element()
    }
}

/// Cached host of the collections / history sidebar: renders
/// [`Workspace::render_sidebar`], re-rendered only when notified (see
/// [`Workspace::sidebar_key`]).
pub struct SidebarView {
    pub(super) workspace: WeakEntity<Workspace>,
}

impl Render for SidebarView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.workspace
            .update(cx, |ws, cx| ws.render_sidebar(cx))
            .unwrap_or_else(|_| div().into_any_element())
    }
}
