---
title: Вставка многострочного cURL в однострочное поле URL
summary: gpui-kit single-line Input выкидывает \n при вставке → `\`+перевод строки становился `\ `; лечится on_paste и эвристикой в tokenize
tags:
  - curl
  - gpui-kit
  - pitfall
files:
  - src/core/curl.rs
  - src/app/request.rs
  - src/app/mod.rs
updated: 2026-10-26T16:15:46.990Z
---

Ловушка: gpui-base `insert_clipboard` для однострочного InputState делает `replace('\n', "")`, поэтому `curl ... \<newline>  -H` приходит в Change как `\  -H` (экранированный пробел) — отсюда «не работает обратный слэш».

Двойная защита:
1. src/app/request.rs: URL-Input имеет `.on_paste(...)` → `Workspace::import_curl` с СЫРЫМ текстом буфера (с переводами строк); вернул true — вставка в поле не происходит.
2. src/core/curl.rs `tokenize`: `\` + пробелы + перевод строки/конец = продолжение; `\` + пробелы перед новым словом (вне слова, или ≥2 пробелов внутри слова) = разделитель (так и выглядит склеенная вставка); `e\ f` с одним пробелом внутри слова остаётся экранированным пробелом. Также: PowerShell-бэктик в конце строки, Windows cmd (`^` продолжение и `^"`, `^\^"` — сначала `unescape_cmd`, потом POSIX-разбор), `$'..'` с \xHH, \uXXXX, \e и т.п.
