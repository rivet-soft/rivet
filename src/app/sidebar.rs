use gpui_kit::component::input::Input;
use gpui_kit::*;

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

    fn sidebar_menu_popover(
        &self,
        target: SidebarMenu,
        id: String,
        collection_ix: Option<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.collection_menu.as_ref() == Some(&target);
        let rows: Vec<(&'static str, &'static str)> = match target {
            SidebarMenu::Collection(_) => vec![
                ("new-request", "Новый запрос"),
                ("new-folder", "Новая папка"),
                ("rename", "Переименовать"),
                ("settings", "Настройки коллекции"),
                ("delete", "Удалить коллекцию"),
            ],
            SidebarMenu::Folder(_, _) => vec![
                ("new-request", "Новый запрос"),
                ("new-folder", "Новая вложенная папка"),
                ("rename", "Переименовать"),
                ("delete", "Удалить папку"),
            ],
            SidebarMenu::Request(_) => {
                vec![("rename", "Переименовать"), ("delete", "Удалить запрос")]
            }
        };
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
            .trigger(
                div()
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
                    .test_support(),
            );
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
            .bg(p.surface)
    }

    fn render_history_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        self.sidebar_shell(self.sidebar_width, cx)
            .child(self.history_panel.clone())
            .into_any_element()
    }

    fn render_collections(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *cx.palette();
        let active = self.active().cloned();

        let mut tree = div()
            .id("collection-tree")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
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
                    let selected = active.as_ref().is_some_and(|a| a.id == r.id);
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
