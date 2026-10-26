---
title: Неопределённые {{vars}} отправляются пустой строкой
summary: resolve() заменяет неопределённые/циклические плейсхолдеры на "", пишет их в missing_vars; UI/MCP предупреждают, но не блокируют
tags:
  - vars
  - decision
files:
  - src/core/vars.rs
  - src/core/runner.rs
  - src/app/mod.rs
  - src/app/request.rs
  - src/mcp/tools.rs
updated: 2026-10-26T16:15:46.952Z
---

Решение пользователя (2026-10-26): переменная, которой нет ни в окружении, ни в коллекции, отправляется ПУСТОЙ строкой + предупреждение. Блокировку запроса (ошибка + «Отправить как есть») пробовали и отвергли — не возвращать.

- src/core/vars.rs `resolve`: после `substitute` оставшиеся `{{..}}` вырезаются `blank_placeholders`; param/header, у которого ключ стал пустым (`{{test111}}=1`), выкидывается. `substitute` сам по себе плейсхолдеры НЕ трогает (им пользуются другие места) — пустые строки только в `resolve`.
- Если URL стал невалидным из-за пустой переменной (`{{baseUrl}}/x`), InvalidUrl несёт исходный шаблон, а не "/x".
- Предупреждения: плашка `missing-vars` в строке запроса (src/app/request.rs, считается через `request_env` = env + переменные коллекции), тост после отправки (`missing_vars_notice` в src/app/mod.rs, из `RunOutput.missing_vars`), в MCP send_request поля `undefined_variables` + `warning` (src/mcp/tools.rs).
- `RunOutput.missing_vars` считается ПОСЛЕ pre-request скрипта, так что переменные, заданные скриптом, не попадают в предупреждение.
- WS (`ws::resolve`) идёт через тот же `vars::resolve` → тоже пустые строки.
