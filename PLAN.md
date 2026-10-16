# Rivet — план функциональности

Rivet — быстрый нативный HTTP‑клиент (Rust + GPUI через `gpui-kit` 0.7.3), построенный вокруг OpenAPI‑спецификации.
Дизайн: claude.ai/design проект «Relay Requests» (кадры 1a–1h).

## Функциональность (готовое приложение)

| # | Возможность | Модуль | Владелец |
|---|---|---|---|
| 1 | Отправка запросов: все методы, query params, headers, body (JSON/Text/Form), auth (Bearer/Basic/API key/Inherit) | `core::http`, `core::vars` | агент HTTP |
| 2 | Переменные `{{var}}` из окружения в URL/params/headers/body/auth, список неопределённых | `core::vars` | агент HTTP |
| 3 | Тайминги (DNS/Connect/TLS/TTFB/Download/Total), размер, версия HTTP, адрес | `core::http` | агент HTTP |
| 4 | Отмена (Esc), таймаут, понятные сетевые ошибки (ECONNREFUSED, ENOTFOUND, TLS…) | `core::http` | агент HTTP |
| 5 | Импорт/экспорт cURL (вставка cURL в поле URL создаёт запрос; «Copy as cURL») | `core::curl` | агент HTTP |
| 6 | Импорт OpenAPI 3 (YAML/JSON): группы по тегам → дерево «Из спецификации», пример тела из схемы | `core::openapi` | агент OpenAPI |
| 7 | Проверка ответа по схеме → расхождения (кадр 1b), «Generate from schema» | `core::schema` | агент OpenAPI |
| 8 | Хранение проекта на диске: окружения, коллекции, история, настройки, открытые вкладки | `core::store` | агент Store |
| 9 | Редактор окружений (rail «Envs»), история (rail «History») | `app::env_panel`, `app::history_panel` | агент Store |
| 10 | Редактор запроса: вкладки Params / Headers / Body / Auth, редактируемые таблицы, Beautify | `app::request_editor` | агент RequestEditor |
| 11 | Просмотр ответа: статус, время, размер, Body (Pretty/Raw/Preview), Headers, Timeline, состояния загрузки/ошибки/пусто, панель расхождений | `app::response_view` | агент Response |
| 12 | Палитра команд ⌘K: поиск запросов и действий | `app::palette` | агент Palette |
| 13 | Сборка окна: вкладки, отправка в фоне, подтверждение DELETE в Prod, ⌘S сохранение, автосохранение, открытие спецификации | `app::mod` + остальное | интегратор (главная сессия) |
| 14 | Самообновление из GitHub Releases: проверка, загрузка с проверкой SHA256SUMS, замена файлов и перезапуск; кнопка над «Настройками» | `core::update`, `app::update_ui` | интегратор |

## Общие правила для всех

- Общая модель данных — `src/core/types.rs`. **Не меняйте её**; если не хватает поля, напишите об этом в отчёте.
- Модули уже зарегистрированы в `src/core/mod.rs` и `src/app/mod.rs` — правьте только свои файлы. `src/ui/mod.rs` можно только дописывать в конец.
- Файл должен **всегда компилироваться** (несколько агентов собирают один checkout). Если сборка падает в чужом файле — подождите и повторите, не правьте его.
- UI: компоненты из `crate::ui`, цвета только из `cx.palette()`, интерактивные элементы — `.id(..)` + `.test_support()`.
- Тесты: `tests/<module>.rs`, стиль как в `tests/workspace.rs`. В тестах не использовать `use gpui_kit::*` (ломает `#[gpui_kit::test]`).
- Колбэки в kit‑компонентах имеют сигнатуру `(…, &mut Window, &mut App)` — используйте `WeakEntity::update`.
- События `InputState` доходят до подписчиков только после завершения текущего `update`.

## Интерфейсы (контракт для интеграции)

### core::vars
```rust
pub fn referenced_vars(text: &str) -> Vec<String>;
pub fn substitute(text: &str, env: Option<&Environment>) -> (String, Vec<String> /*missing*/);
pub fn resolve(req: &RequestSpec, env: Option<&Environment>) -> Result<ResolvedRequest, SendError>;
```
### core::http
```rust
pub struct CancelToken(/* Arc<AtomicBool> */);  // Clone + Default, .cancel(), .is_cancelled()
pub fn execute(req: &ResolvedRequest, settings: &Settings, cancel: &CancelToken) -> Result<ResponseData, SendError>; // blocking
pub fn format_size(bytes: usize) -> String;      // "1.2 KB"
pub fn format_duration(d: Duration) -> String;   // "148 ms", "1.4 s"
pub fn status_text(code: u16) -> &'static str;   // "Created"
```
### core::curl
```rust
pub fn parse_curl(cmd: &str) -> Option<RequestSpec>;
pub fn to_curl(req: &ResolvedRequest) -> String;
```
### core::openapi
```rust
pub struct Spec { pub title: String, pub version: String, pub servers: Vec<String>, pub groups: Vec<SpecGroup>, /* + internal doc */ }
impl Spec {
    pub fn load(path: &Path) -> anyhow-free Result<Spec, String>;
    pub fn parse(text: &str) -> Result<Spec, String>;            // YAML or JSON
    pub fn find(&self, method: Method, path: &str) -> Option<&SpecEndpoint>;
    pub fn response_schema_label(&self, r: &SpecRef, status: u16) -> Option<String>; // "responses.201 → $ref Order"
    pub fn validate(&self, r: &SpecRef, status: u16, body: &serde_json::Value) -> Option<Vec<Mismatch>>; // None = no schema
    pub fn example_body(&self, r: &SpecRef) -> Option<String>;
}
```
`examples/shop-api.yaml` — демо‑спецификация «Shop API 2.3.0» (orders/products/customers/auth), как в дизайне.

### core::store
```rust
pub fn default_path() -> PathBuf;                        // ~/Library/Application Support/Rivet/project.json
pub fn load(path: &Path) -> Result<Project, String>;
pub fn save(path: &Path, project: &Project) -> Result<(), String>; // atomic write
pub fn load_or_default(path: &Path) -> Project;         // seed: Dev/Prod envs, spec = examples/shop-api.yaml
pub fn push_history(project: &mut Project, entry: HistoryEntry); // newest first, cap 500
```

### app::request_editor
```rust
pub struct RequestEditor;  // Entity
impl RequestEditor {
    pub fn new(spec: RequestSpec, window, cx) -> Self;
    pub fn set_spec(&mut self, spec: RequestSpec, window, cx);
    pub fn spec(&self) -> &RequestSpec;
}
pub enum RequestEditorEvent { Changed, GenerateBody }   // impl EventEmitter
```
### app::response_view
```rust
pub enum ResponseState { Empty, Loading { host: String }, Done { response: ResponseData, schema_label: Option<String>, mismatches: Vec<Mismatch> }, Failed(SendError) }
pub struct ResponseView;  // Entity
impl ResponseView { pub fn new(window, cx) -> Self; pub fn set_state(&mut self, s: ResponseState, cx); }
pub enum ResponseViewEvent { Retry, Cancel, UpdateSchema, GoToSchema }
```
### app::palette
```rust
pub struct PaletteItem { pub id: String, pub title: String, pub subtitle: String, pub method: Option<Method>, pub kbd: Option<String> }
pub struct CommandPalette;  // Entity, focusable
impl CommandPalette { pub fn new(items: Vec<PaletteItem>, window, cx) -> Self; pub fn set_items(..); }
pub enum PaletteEvent { Picked(String /*id*/), Dismissed }
```
### app::env_panel / app::history_panel
```rust
pub struct EnvPanel;     // Entity: list of envs + editable vars, add/remove env, production flag, color
impl EnvPanel { pub fn new(envs: Vec<Environment>, active: usize, window, cx) -> Self; pub fn envs(&self) -> &[Environment]; }
pub enum EnvPanelEvent { Changed, Activated(usize) }
pub struct HistoryPanel; // Entity: newest-first list, filter, method/status/time
impl HistoryPanel { pub fn new(window, cx) -> Self; pub fn set_entries(&mut self, Vec<HistoryEntry>, cx); }
pub enum HistoryPanelEvent { Open(HistoryEntry), Clear }
```
