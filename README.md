<p align="center">
  <img src="assets/app-icon.png" width="96" alt="Rivet icon">
</p>

<h1 align="center">Rivet</h1>

<p align="center">
  Быстрый нативный HTTP‑клиент для API.<br>
  Rust + GPUI · macOS, Windows, Linux · OpenAPI, Postman, скрипты <code>pm.*</code>
</p>

<p align="center">
  <img src="docs/screenshot.png" alt="Rivet: запрос, ответ, скрипты и коллекции" width="100%">
</p>

---

## Почему Rivet

- **Мгновенный.** Нативный интерфейс на GPU (GPUI, движок редактора Zed), без Electron. Большие ответы виртуализированы и не тормозят.
- **Вокруг спецификации.** Импорт OpenAPI 3.x / Swagger 2.0 и Postman‑коллекций, проверка ответа по схеме с переходом к строке расхождения, генерация тела из схемы.
- **Автоматизация.** Pre‑request и Post‑response скрипты с Postman‑совместимым `pm.*` и тестами `pm.test` / `pm.expect` на встроенном JS‑движке [Boa](https://boajs.dev) (чистый Rust).
- **Прозрачный.** Timeline показывает весь путь запроса: каждый редирект (откуда → куда, статус) и фазы DNS / Connect / TLS / TTFB / Download.
- **Всё локально.** Проект хранится у вас: `~/Library/Application Support/Rivet/project.json`, без аккаунтов и облака.

## Быстрый старт

```bash
cargo run --release          # приложение
cargo run --bin gallery      # витрина UI‑компонентов
cargo test                   # все тесты
```

При первом запуске открывается пустой проект. Демо‑спецификацию `examples/shop-api.yaml` можно импортировать через ⌘O.
Если раньше вы пользовались Relay (прежнее название), проект из `Application Support/Relay` переносится автоматически; старый файл остаётся как резервная копия.

## Возможности

### Запросы
- GET / POST / PUT / PATCH / DELETE / HEAD / OPTIONS; Params, Headers, Body (none, form‑data с файлами, x‑www‑form‑urlencoded, raw JSON с подсветкой, Text, binary), Auth (Bearer / Basic / API Key / Inherit from spec).
- Переменные `{{var}}` из коллекции и активного окружения (коллекция в приоритете) — в URL, params, headers, body и auth, с подсветкой и предупреждением о неопределённых. Встроенные: `{{$uuid}}`, `{{$timestamp}}`, `{{$randomInt}}`.
- cURL: вставьте `curl …` в поле URL — получится запрос; «Copy cURL» копирует активный.
- Вкладки с меню действий, принудительное закрытие, перетаскивание запросов между коллекциями и папками.

### Скрипты
- Вкладка **Scripts**: Pre‑request (меняет запрос и переменные перед отправкой) и Post‑response (разбор ответа, тесты), сниппеты в один клик.
- `pm.environment`, `pm.collectionVariables`, `pm.variables`, `pm.request`, `pm.response`, `pm.test`, `pm.expect` (chai‑подобные проверки), legacy `responseBody` / `tests[...]` / `postman.*`, `console.log`.
- Результаты во вкладках ответа **Tests N/M** и **Console**; ошибка pre‑request скрипта останавливает отправку и показывает строку.
- Скрипты из Postman‑коллекций импортируются вместе с запросами.

```js
// Post-response: сохранить значение для следующих запросов
const json = pm.response.json();
pm.collectionVariables.set("searchId", json.searchId);
pm.test("Status is 200", () => pm.response.to.have.status(200));
```

### HTTP/2 и WebSocket
- HTTP/2 через ALPN (h2 в приоритете), h2c для `http://`, выбор Auto / HTTP/1.1 / HTTP/2 в настройках; HTTP/1.1 и HTTP/2 через CONNECT‑прокси; gzip / deflate / br.
- **WebSocket** (`ws://`, `wss://`): вкладка «+ → WebSocket», Connect / Disconnect, отправка Text / JSON / Binary (hex), ping, шаблоны сообщений, лента входящих и исходящих с подробностями, заголовки и auth рукопожатия. WS‑запросы сохраняются в коллекции и попадают в историю.

### Ответ
- Статус, время, размер; Body: Pretty / Raw / Preview (с поиском ⌘F) / **Browser** (настоящий WebKit с CSS и JS).
- Весь текст во вкладках ответа выделяется мышью и копируется ⌘C.
- Headers, **Timeline** с полным путём редиректов, Tests, Console.
- Проверка по связанной спецификации: панель расхождений со схемой и переход к строке.

### Коллекции, окружения, история
- Несколько коллекций, папки, переменные коллекции; обновление коллекции из исходной спецификации; удаление коллекции (исходный файл импорта не изменяется).
- Окружения с цветом, секретами и флагом **Production** — красная рамка и подтверждение перед POST / PUT / PATCH / DELETE.
- История с поиском и повтором запроса.
- Палитра команд ⌘K: нечёткий поиск по запросам, командам и окружениям.

### Интерфейс
- Тёмная и светлая темы, системное меню на macOS и меню в окне на Windows / Linux, иконка в Dock и в `.exe`.

## Горячие клавиши

| macOS | Windows / Linux | Действие |
|---|---|---|
| ⌘↵ | Ctrl+Enter | Отправить |
| ⌘. | Ctrl+. | Отменить отправку |
| ⌘K / ⌘P | Ctrl+K / Ctrl+P | Палитра команд |
| ⌘T / ⌘W | Ctrl+T / Ctrl+W | Новая / закрыть вкладку |
| ⌥⌘W | Ctrl+Alt+W | Принудительно закрыть вкладку |
| Ctrl+Tab / Ctrl+Shift+Tab | Ctrl+Tab / Ctrl+Shift+Tab | Следующая / предыдущая вкладка |
| ⌘S | Ctrl+S | Сохранить запрос |
| ⌘⇧C | Ctrl+Shift+C | Скопировать как cURL |
| ⌘O | Ctrl+O | Импорт OpenAPI, Swagger или Postman |
| ⌘R | Ctrl+R | Обновить связанные спецификации |
| ⌘E | Ctrl+E | Следующее окружение |
| ⌘1…⌘4 | Ctrl+1…Ctrl+4 | Запросы / Окружения / История / Коллекции |
| ⌘B | Ctrl+B | Показать / скрыть боковую панель |
| ⌘⇧L | Ctrl+Shift+L | Сменить тему |

## Сборка для Windows

Релиз собирается на Windows командой `./scripts/build-windows.ps1` (PowerShell) или вручную через GitHub Actions `Windows build`. Архив появится в `target/windows-release/Rivet-<версия>-windows-x64.zip` (или в артефактах Actions); распакуйте и запустите `Rivet.exe`.

Быстрая проверочная сборка с macOS: `brew install mingw-w64`, затем `./scripts/build-windows-debug.sh` (архив с суффиксом `-debug`). Релизный профиль требует компилятора DirectX‑шейдеров из Windows SDK, поэтому собирается на Windows.

## Устройство

- `src/core` — без UI: модель данных (`types.rs`), конвейер отправки (`runner.rs`: pre‑скрипт → переменные → HTTP → post‑скрипт), HTTP/1.1 и HTTP/2 (`http/` на hyper + rustls), WebSocket (`ws.rs`), скрипты (`script.rs` + `script_prelude.js` на Boa), переменные (`vars.rs`), cURL, OpenAPI и схемы, импорт Postman, хранилище (`store.rs`), общий Tokio‑рантайм (`rt.rs`).
- `src/ui` — набор компонентов в стиле дизайна Rivet.
- `src/app` — главное окно `Workspace` и панели: редактор запроса, ответ, окружения, коллекции, настройки, история, палитра.
- Ядро не зависит от интерфейса, поэтому тот же `core::runner` используется окном и (в планах) MCP‑сервером.

## В работе

- MCP‑сервер, чтобы ИИ‑ассистенты могли управлять Rivet: запускать запросы, читать ответы, менять переменные.

## Ограничения

- WebSocket не использует настройку Proxy; корневые сертификаты — из набора webpki‑roots, а не системного хранилища.
- HTTP/2 без повторного использования соединений (каждый запрос открывает новое).
- Время в истории показывается в UTC.
- Скрипты: нет `pm.sendRequest`, `require`/CryptoJS и скриптов уровня коллекции.
- Postman `formdata` и бинарные тела импортируются; файлы, на которые ссылается коллекция, должны быть доступны на диске.

## Скриншот для README

Картинка в начале берётся из `docs/screenshot.png`. Чтобы обновить: откройте Rivet, нажмите ⌘⇧4, затем Пробел и кликните по окну — сохраните файл как `docs/screenshot.png`.
