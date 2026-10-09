# Outry — план

API-клиент, где запросы — текстовые файлы в репозитории, а коллекция собирается сама из кода.
Аналог Insomnia/Postman, но без облака и без экспорта/импорта коллекций: правда — в `api/*.outry` (раньше `*.http`).

## Архитектура

```
outry/
├── crates/outry-core   вся логика, без UI: формат, переменные, окружения, секреты, HTTP, save/assert
├── crates/outry-cli    бинарь `outry` поверх core — терминал и CI
├── app/                Tauri 2 + React + TS
│   └── src-tauri       команды Tauri → outry-core, слежение за файлами (notify), автообновление
├── examples/api        пример проекта (httpbin.org)
└── .github/workflows   ci.yml, release.yml
```

GUI и CLI не могут разъехаться: оба вызывают одни и те же `Runner::run` / `parse`.
Крейт назван `outry-core`, а не `core`: имя `core` занято стандартной библиотекой Rust.

### Разрешение переменных (приоритет сверху вниз)

1. `--var name=value` (CLI) / ручные значения
2. значения из `save` / `> save` (сохраняются между запусками в `~/.local/share/outry/state/`, не в репо)
3. переменные процесса `OUTRY_<NAME>` — секреты в CI
4. `[env.<name>]` из `env.toml`, затем общие `[vars]`
5. системное хранилище паролей (`keyring`): `outry secret set token --env dev`

## Этапы

Сделанное (этапы 0–5: каркас, core, CLI, desktop, переменные, импорт из Go) — в git-истории и README.

### 1. Свой формат запросов `*.outry` — приоритет
C-подобный, декларативный, простой без знания Go: JSON-тело, выражения как в JS/Java/C#, запрос — как функция.
Решаем до LSP и VS Code-расширения, иначе их придётся переделывать.

```
// Create order
// Creates an order for the current user.
POST /orders/{shop} {
  only: [dev, staging]

  query { page: 1 }

  headers {
    Authorization: "Bearer ${Login().body.token}"
    Idempotency-Key: uuid()
  }

  body {
    customer: CreateUser(name: "Bob").body.id,
    items: [{ sku: "A-1", qty: 2 }],
  }

  expect {
    status == 201
    body.items.length == 1
    headers.Location.startsWith("/orders/")
    body matches Order
    GetOrder(id: body.id).body.status == "new"
  }

  save order_id = body.id
}

flow Checkout {
  order = CreateOrder(shop: "main")
  Pay(order: order.body.id)
  expect { GetOrder(id: order.body.id).body.status == "paid" }
}
```

Правила:
- `МЕТОД /путь` — весь минимальный файл (`GET /health`); блок `{ … }` по необходимости. `{{base}}` подставляется сам,
  `{shop}` в пути — переменная/параметр (та же запись, что в chi/net/http — импорт кладёт путь как есть)
- `//` — комментарии; первая строка над запросом — его имя (`Create order` → `CreateOrder`), остальные — описание
- `body` — JSON5: чистый JSON из DevTools/Swagger вставляется как есть; ключи без кавычек, висячие запятые,
  сокращение `{ email, password }`; `form`, `multipart`, `file("./payload.json")` для остального
- переменные — голые имена (`customer: user`), в строках `"${user}"`; `secret.password`, `uuid()`, `now()`, `randomInt(1, 10)`
- `expect` — по выражению на строку: `== != < <= > >= && || !`, `.length`, `.startsWith()`, `.contains()`,
  `matches /re/`, `in [..]`, `typeof`; при падении видны обе стороны (`body.total > 0 — got 0`)
- `body matches Order` / `matches { id: string, total: number }` — форма ответа: Go-структура из импорта,
  JSON Schema или форма прямо в файле
- `save name = выражение` — значение для следующих запросов (замена `> save`)
- `only: [dev, staging]` — запрос не уйдёт в другие env, в т.ч. при косвенном вызове; `confirm: true` — спросить перед отправкой
- `poll body.status == "paid" every 1s for 30s` — опрос асинхронных операций

Вызовы запросов:
- запрос — функция: `params { email: "…" }` с умолчаниями + path-параметры; вызов `Login(email: "a@b.c")`, результат — ответ
- вызовы только как значения: в `headers`, `body`, `query`, `expect`, `flow`; никаких `if`, циклов, своих функций
- кеш на прогон: тот же вызов с теми же аргументами выполняется один раз; `Login().fresh()` — заново
- имена уникальны в проекте без импортов, при конфликте — с папкой (`users.Create()`); циклы и конфликты — ошибка `outry check`
- упавший `expect` вызванного запроса роняет вызывающий, ошибка с цепочкой: `CreateOrder → Login: status == 200 — got 401`
- `flow Name { … }` — сценарий; `outry run Checkout`, кнопка в GUI; вкладка «Trace» с вызванными запросами

Задачи:
- [x] Спека: `docs/…/reference/outry-format.mdx` (EN + RU), подсветка `docs/src/outry.tmLanguage.json`;
  открытые вопросы закрыты (`run <каталог>` запускает всё, cookies на прогон + `cookies.x`, `cache:` в state-файле)
- [x] `crates/outry-core/src/lang/`: парсер с позициями (`Span` у узлов, комментарии отдельно), вычислитель,
  формы, `outry check` (синтаксис, имена с «did you mean», аргументы, формы, циклы) — `path:line:col`
- [x] Вызовы запросов, кеш на прогон, `fresh`, циклы, `flow`, `poll`, `only`, `confirm` (`--yes`), cookies;
  `outry run` по файлу, каталогу, имени и `файл:строка`; trace вызовов в выводе CLI
- [x] `cache: 30m` между прогонами (state-файл), `multipart`, `schema("…")` (подмножество JSON Schema, `lang/schema.rs`)
- [x] `outry check --env prod`: `only`, неизвестные окружения и недостающие переменные без отправки (`lang/env_check.rs`)
- [x] `outry fmt [--check]` — канонический вид из дерева с комментариями; элемент, который не печатается
  без потерь, остаётся как был (проверка повторным разбором)
- [x] `.http` — режим совместимости + `outry convert [--dry-run] [--rm]`
- [x] Генерация `*.outry` в `import go`: имя и описание из doc-комментария, `handler:`, query → `params`,
  тело из структуры; сопоставление по `handler`, затем по методу и пути
- [x] GUI: запуск `*.outry` (прогон на окружение, элемент под курсором, ▶ на полях, `confirm` — диалог),
  подсветка, `outry check` на лету, автодополнение, вкладка Trace, итог сценария
- [x] `examples/api` (вызовы, форма, сценарий), README, docs (EN + RU), `outry init` — на новый формат

### 2. Коллекция, которая не устаревает (киллер-фича)
Питч: «одна строка в CI — и API-тесты больше не разъедутся с кодом». Bruno/Postman о коде ничего не знают.
Не просто «сходится / нет», а что поменялось в Go и что поправить в `.outry` — с автоправкой.
- [x] Поле `handler: users.GetUser` в запросе: импорт пишет его, сопоставление по хендлеру переживает смену пути и метода
  (без `handler` — по методу и пути, `import/mod.rs`)
- [x] Сравнение по полям: путь/метод, переименованный path-параметр, `required`-поле тела отсутствует в `body`,
  лишнее поле, сменился тип (`"25"` vs `int`), новые query/заголовки, удалённый роут, новый роут
  (`import/diff.rs` → `Existing::changes`; негативные тесты со `status` 4xx — только путь и метод)
- [x] Тип ответа: `json.Encode` / `c.JSON` / `writeJSON` → `shape` в `shapes.outry` (`go/describe.rs`), новые запросы
  получают `body matches Order`; shape сравниваются со структурами (строже кода — можно), `--fix` переписывает
  несовместимые поля (`import/shapes.rs`)
- [x] `outry import go --check`: отчёт по файлам со ссылкой на строку в Go, exit code 1 при ошибках (warning — нет)
- [x] `--fix` — правки с diff (затронутый запрос печатается заново `fmt`), `--fix --dry-run`; `--prune` удаляет
  файлы, где все запросы — к пропавшим роутам
- [x] `--format github` — аннотации на строках `.outry`/Go; `--format json` / `--json`
- [x] GUI: во вкладке Routes бейдж «changed», diff и «Apply» на каждое изменение, «Fix N», «Remove N files»
- [x] LSP: диагностика в `.outry` («path changed in code: …») + Quick Fix — `outry lsp` (этап 3)
- [x] `.http`: сопоставление по методу и пути; сравниваются тело (если валидный JSON), query и заголовки, без правок
- [x] `body matches Order` по Go-структуре: импорт генерирует shape из типа ответа

### 3. Редакторы без GUI: `outry lsp`
Протокол — `crates/outry-cli/src/lsp.rs` (`lsp-server` + `lsp-types`), подсказки — `lang/ide.rs`,
области видимости имён — `lang/scope.rs` (общие с `check --env`).
- [x] `outry lsp` (stdio, `lsp-server`) внутри того же бинаря; настройки `env`, `keyring`, `import`, `goDir`
- [x] Диагностика как `outry check` + `check --env` текущего окружения (предупреждения); автодополнение
  переменных (`Vars::list`), встроенных функций, имён запросов с аргументами, полей, форм, окружений в `only`
- [x] Расхождения с кодом (`import::plan_with` по текстам из редактора) как диагностика со ссылкой на Go,
  `Edit` — как Quick Fix и «Fix all»; «did you mean» — тоже Quick Fix
- [x] Hover: значение и источник переменной (секреты замаскированы), сигнатура вызываемого запроса, форма
- [x] Go to definition: переменная → строка в `env.toml` и `save`, `Login()` → его файл, `Order` → объявление
  → Go-структура, `handler:` → роут
- [x] Code lens «Send» / «Run flow» (и то же в code actions — для Helix) → ответ в
  `~/.cache/outry/responses/<Name>.http` через `window/showDocument`; `confirm` — `showMessageRequest`
- [x] Грамматика tree-sitter `editors/tree-sitter-outry` + запросы подсветки (Neovim/Zed и Helix), проверка в CI
- [x] Страница в docs: настройка редакторов (Neovim, Helix, остальные)
- [ ] Расширение для Zed (грамматика + запуск `outry lsp`) — вместе с этапом 4

### 4. VS Code-расширение
`editors/vscode`: LSP-клиент + интерфейс; логика вся в `outry`, своих правил в расширении нет. Сервер знает
о таком клиенте по `experimental.outryUi` (`outry/state`, `outry/didChange`, команды расширения в code lens).
- [x] Платформенные VSIX с бинарём `outry` внутри (из архивов CLI релиза) + универсальный без него — в
  `release.yml`; публикация в Marketplace + Open VSX, когда заданы `VSCE_PAT` / `OVSX_PAT`
- [x] Code lens «▶ Send», «in…» (другое окружение), «Copy as curl» (`outry.curl`, `Run::resolve_item`);
  ответ в webview — ResponseView / FlowView / BodyViewer из `app/src` (vite alias `@app`, типы — `app/src/types.ts`)
- [x] Окружение в строке состояния; боковая панель: дерево запросов и сценариев, переменные
- [x] Интеграционный тест в настоящем VS Code (`npm test`, в CI под xvfb)
- [ ] Боковая панель: Routes с синхронизацией (пока — диагностика и Quick Fix), история ответов
- [ ] Публикация: создать издателя `outry` в Marketplace и namespace в Open VSX, секреты в репозитории

### 5. MCP для AI-агентов
- [ ] `outry mcp` (stdio): «список запросов», «выполнить запрос/flow в env», «показать переменные» поверх `Runner`/`Vars`
- [ ] Секреты подставляются внутри outry и не попадают к агенту (значения из keyring/`secrets` маскируются)
- [ ] Страница в docs: подключение к Claude Code / Cursor

### 6. Распространение
- [ ] install.sh и `outry update` для macOS (архивы уже есть) и Windows (`install.ps1`, zip)
- [ ] Пакетные менеджеры для CLI: Homebrew tap, AUR, COPR, Scoop/winget
- [ ] Подпись и нотаризация macOS, подпись Windows-установщика

### Потом
- CI-обвязка для этапа 2: JUnit-отчёт (`--report junit.xml`); рецепт для CI в docs (GitHub Actions, GitLab)
  и готовый `outry-action`
- Импорт роутов из других языков (FastAPI, Express, Spring) — когда Go-история заработает
- Импорт из Postman / Insomnia / OpenAPI в `*.outry`
- GraphQL, WebSocket — не раньше этапов 1–4

## Релизы и автообновление

**Один раз:**
1. Ключ подписи обновлений уже сгенерирован в `~/.tauri/routy.key` (публичный ключ — в `app/src-tauri/tauri.conf.json`).
   Сделайте резервную копию: если ключ потерять, установленные приложения больше не примут обновление.
   Лучше перегенерировать с паролем до первого релиза: `cargo tauri signer generate -w ~/.tauri/routy.key -f`
   и обновить `plugins.updater.pubkey`.
2. GitHub → Settings → Secrets → Actions:
   `TAURI_SIGNING_PRIVATE_KEY` = содержимое `~/.tauri/routy.key`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` = пароль (или пусто).
3. Если репозиторий не `github.com/1rowvy/outry` — поправить `endpoints` в `tauri.conf.json` и `repository` в `Cargo.toml`.

**Каждый релиз:**
```sh
scripts/release.sh 0.2.0
git commit -am "release v0.2.0" && git tag v0.2.0 && git push && git push origin v0.2.0
```
`release.yml` создаёт черновик → собирает приложения и CLI → публикует. Только после публикации
`releases/latest/download/latest.json` указывает на новую версию, и установленные приложения предлагают обновиться.

**Как работает обновление:** при старте приложение запрашивает `latest.json`, сравнивает версии,
скачивает архив своей платформы, проверяет подпись по `pubkey` и ставит. Обновляются:
macOS (`.app.tar.gz`), Windows (NSIS-установщик), Linux — только AppImage (`.deb`/`.rpm` обновляются пакетным менеджером).

**Не подписано:** macOS-сборки без Apple Developer ID — Gatekeeper ругается при первом запуске
(`xattr -dr com.apple.quarantine /Applications/Outry.app`). Секреты для подписи перечислены в шапке `release.yml`.
