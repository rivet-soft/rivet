---
title: Валидация HTTP-заголовков
summary: "core/header_check.rs: ошибки блокируют отправку (SendError::InvalidHeader), предупреждения — только подсветка; UTF-8 значения отправляются через HeaderValue::from_bytes"
tags:
  - headers
  - validation
files:
  - src/core/header_check.rs
  - src/core/vars.rs
  - src/core/http/hop.rs
  - src/app/request_editor.rs
  - src/app/response_view.rs
updated: 2026-10-26T16:54:55.998Z
---

- Правила в src/core/header_check.rs: имя — только tchar RFC 9110 (пробел, `:`, управляющие, кириллица, `{` → ошибка; пустое имя при значении → ошибка). Значение: CR/LF, NUL, управляющие (кроме TAB) → ошибка; пробелы по краям и не-ASCII → предупреждение.
- `vars::resolve` в конце вызывает `check_headers` по ИТОГОВЫМ заголовкам (после подстановки и auth) → `SendError::InvalidHeader{name,message}`; если заголовок шаблонный, к сообщению добавляется «(после подстановки переменных)». Runner/MCP/collection_run получают это автоматически (MCP печатает `e.to_string()`).
- В таблице Headers (`RequestEditor::header_issues`) имя проверяется с замаскированными `{{..}}` (`vars::mask_placeholders`), иначе `{{hdr}}` считался бы ошибкой. Значки `key-issue`/`value-issue` с Tooltip, рамка danger/warn.
- Ловушка: hyper `builder.header(k, &str)` = `HeaderValue::from_str`, он отвергает не-ASCII с невнятной ошибкой. В src/core/http/hop.rs значения идут через `HeaderValue::from_bytes` (obs-text разрешён).
