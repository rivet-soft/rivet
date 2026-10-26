---
title: Разворачивание длинного значения в таблице KV
summary: Фокус на длинной ячейке значения открывает TextareaState поверх строк; фокус определяется при рендере, т.к. InputEvent::Focus в тестах не приходит
tags:
  - ui
  - gpui
  - pitfall
files:
  - src/app/request_editor.rs
updated: 2026-10-26T16:54:56.071Z
---

- `RequestEditor::sync_expanded_value` (вызывается из render) смотрит `focus_handle.is_focused(window)` у ячеек значения; если текст шире ячейки (`mono_text_width`, ширины ячеек пишет canvas в `value_widths`) — через `defer_in` создаёт `ExpandedValue` (TextareaState auto_grow 1..8, max_h 200, deferred(anchored()) той же ширины, id `value-expanded`). Потеря фокуса → `collapse_value` переносит текст в однострочный InputState.
- ЛОВУШКА: в headless-тестах тестовое окно `is_active() == false`, и GPUI не вызывает focus-слушатели → `InputEvent::Focus/Blur` не приходят. Поэтому фокус проверяется при рендере, а не по событиям.
- Enter в развёрнутом редакторе завершает редактирование (перевод строки вырезается: значение однострочное).
