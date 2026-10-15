//! Native application menus. Items dispatch the same actions as the UI and shortcuts.

use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
use gpui_kit::{Menu, MenuItem, OsAction};

use super::{
    CancelRequest, CloseTab, CopyCurl, NewTab, NextTab, OpenPalette, OpenSpec, PrevTab, Quit,
    ReloadSpec, SaveProject, SaveRequest, SendRequest, ShowCollections, ShowEnvs, ShowHistory,
    ShowRequests, ShowSettings, ToggleEnv, ToggleSidebar, ToggleTheme,
};

pub fn app_menus() -> Vec<Menu> {
    vec![
        Menu::new("Rivet").items([
            MenuItem::action("Палитра команд…", OpenPalette),
            MenuItem::separator(),
            MenuItem::action("Выход из Rivet", Quit),
        ]),
        Menu::new("Файл").items([
            MenuItem::action("Новый запрос", NewTab),
            MenuItem::action("Импортировать OpenAPI, Swagger или Postman…", OpenSpec),
            MenuItem::action("Обновить связанные спецификации", ReloadSpec),
            MenuItem::separator(),
            MenuItem::action("Сохранить запрос", SaveRequest),
            MenuItem::action("Сохранить проект", SaveProject),
            MenuItem::separator(),
            MenuItem::action("Закрыть вкладку", CloseTab),
        ]),
        Menu::new("Правка").items([
            MenuItem::os_action("Отменить", Undo, OsAction::Undo),
            MenuItem::os_action("Повторить", Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Вырезать", Cut, OsAction::Cut),
            MenuItem::os_action("Копировать", Copy, OsAction::Copy),
            MenuItem::os_action("Вставить", Paste, OsAction::Paste),
            MenuItem::os_action("Выделить всё", SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("Запрос").items([
            MenuItem::action("Отправить", SendRequest),
            MenuItem::action("Отменить отправку", CancelRequest),
            MenuItem::separator(),
            MenuItem::action("Скопировать как cURL", CopyCurl),
            MenuItem::action("Следующее окружение", ToggleEnv),
        ]),
        Menu::new("Вид").items([
            MenuItem::action("Запросы", ShowRequests),
            MenuItem::action("Коллекции", ShowCollections),
            MenuItem::action("Окружения", ShowEnvs),
            MenuItem::action("История", ShowHistory),
            MenuItem::action("Настройки", ShowSettings),
            MenuItem::separator(),
            MenuItem::action("Показать или скрыть боковую панель", ToggleSidebar),
            MenuItem::action("Сменить тему", ToggleTheme),
            MenuItem::separator(),
            MenuItem::action("Следующая вкладка", NextTab),
            MenuItem::action("Предыдущая вкладка", PrevTab),
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use gpui_kit::MenuItem;

    use super::app_menus;

    #[test]
    fn native_menus_expose_file_edit_request_and_view_commands() {
        let menus = app_menus();
        let names: Vec<_> = menus.iter().map(|menu| menu.name.as_ref()).collect();
        assert_eq!(names, ["Rivet", "Файл", "Правка", "Запрос", "Вид"]);

        let file_actions: Vec<_> = menus[1]
            .items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Action { action, .. } => Some(action.name()),
                _ => None,
            })
            .collect();
        assert!(file_actions.contains(&"rivet::NewTab"));
        assert!(file_actions.contains(&"rivet::OpenSpec"));
        assert!(file_actions.contains(&"rivet::SaveProject"));
        assert!(file_actions.contains(&"rivet::CloseTab"));


        assert!(menus[2].items.iter().any(|item| matches!(
            item,
            MenuItem::Action {
                os_action: Some(_),
                ..
            }
        )));
    }
}
