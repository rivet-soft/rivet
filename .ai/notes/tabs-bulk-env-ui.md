---
title: Вкладки (DnD, sticky +), bulk-edit, «Создать окружение»
summary: Как устроены drag&drop вкладок, липкий «+», bulk-edit Params/Headers и кнопка создания окружения в тайтлбаре
tags:
  - ui
  - tabs
  - bulk-edit
  - env
files:
  - src/ui/tabs.rs
  - src/app/request.rs
  - src/app/mod.rs
  - src/core/bulk.rs
  - src/app/request_editor.rs
  - src/ui/overlay.rs
  - src/app/titlebar.rs
updated: 2026-10-26T16:15:47.003Z
---

- DnD вкладок: `Tab::draggable(ix)` + `on_tab_drop` (payload `TabDrag`, src/ui/tabs.rs), обработчик `Workspace::move_tab` (src/app/mod.rs) — переставляет `tabs`, переносит `active_tab`/`close_confirm` по id запроса, сбрасывает `tab_menu`. Порядок в project.open_tabs пишет `persist_project` из `self.tabs`, отдельно сохранять не нужно. Тест — `drag_to(("tab",0),("tab",2))`.
- Липкий «+» (render_tabs в src/app/request.rs): обёртка полосы `flex_initial().min_w_0()` (не flex_1!) + «+» `flex_none` + filler `flex_1` с нижней границей. Пока влезает — «+» сразу за последней вкладкой, при переполнении полоса сжимается и скроллится, «+» у правого края. id `new-tab-area`.
- Bulk-edit: чистые функции в src/core/bulk.rs (`to_text`/`from_text`, `//` = выключено, ключ до первого `:`, description/file переносятся из исходных строк по ключу). В RequestEditor `bulk: Vec<BulkEdit>` (EditorState на таблицу), кнопка `bulk-toggle` в шапке только для Params/Headers. Подписка игнорирует Change, если текст == to_text(текущий список) — так `set_spec` (обязан ничего не эмитить) не порождает Changed.
- «Создать окружение»: `EnvPopover::on_create` (строка `env-create`, всегда видна; при пустом списке — подпись «Окружений пока нет») → `Workspace::create_environment`: синхронизирует EnvPanel из project, `add_env`, `set_rail(Environments)`. Новое окружение становится активным только если их не было (active_env=0).
