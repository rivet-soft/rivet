---
title: "Лаги при наборе: сайдбар как кэшируемый view"
summary: Каждая клавиша перерисовывала весь Workspace, включая дерево коллекций; сайдбар вынесен в SidebarView с .cached() и ключом sidebar_key
tags:
  - perf
  - gpui
  - pitfall
files:
  - src/app/sidebar.rs
  - src/app/mod.rs
updated: 2026-10-26T16:54:56.051Z
---

Замеры (debug, 300 запросов в коллекциях): кадр рос линейно с числом строк дерева, вкладки/история почти не влияли. Ввод в любое поле → cx.notify() Workspace → полная перерисовка.

Решение: `SidebarView` (src/app/sidebar.rs) рендерит `Workspace::render_sidebar` через WeakEntity и встраивается как `sidebar_view.cached(style)` в Workspace::render. Перерисовка только при смене `Workspace::sidebar_key` (хэш rail, ширины, фильтра, меню, свёрнутых узлов, активного id, темы, id/name/method/folder/url запросов коллекций). Результат: 161 → 62 мс/клавишу в debug.

ЛОВУШКА: если сайдбар начнёт зависеть от нового поля Workspace — добавь его в `sidebar_key`, иначе UI не обновится. Также меню «···» строится только у открытой строки.

Как мерить: gpui-kit `render_frame`/`input` делают window.refresh() и обходят кэш — для честного замера `window.simulate_input("a")` + `run_until_parked()`. Профиль — `sample <pid>` на тестовом бинаре.
