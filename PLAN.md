# Routy — план

API-клиент, где запросы — текстовые файлы в репозитории, а коллекция собирается сама из кода.
Аналог Insomnia/Postman, но без облака и без экспорта/импорта коллекций: правда — в `api/*.routy` (раньше `*.http`).

## Архитектура

```
routy/
├── crates/routy-core   вся логика, без UI: формат, переменные, окружения, секреты, HTTP, save/assert
├── crates/routy-cli    бинарь `routy` поверх core — терминал и CI
├── app/                Tauri 2 + React + TS
│   └── src-tauri       команды Tauri → routy-core, слежение за файлами (notify), автообновление
├── examples/api        пример проекта (httpbin.org)
└── .github/workflows   ci.yml, release.yml
```

GUI и CLI не могут разъехаться: оба вызывают одни и те же `Runner::run` / `parse`.
Крейт назван `routy-core`, а не `core`: имя `core` занято стандартной библиотекой Rust.

### Разрешение переменных (приоритет сверху вниз)

1. `--var name=value` (CLI) / ручные значения
2. значения из `save` / `> save` (сохраняются между запусками в `~/.local/share/routy/state/`, не в репо)
3. переменные процесса `ROUTY_<NAME>` — секреты в CI
4. `[env.<name>]` из `env.toml`, затем общие `[vars]`
5. системное хранилище паролей (`keyring`): `routy secret set token --env dev`

## Этапы

Сделанное (этапы 0–5: каркас, core, CLI, desktop, переменные, импорт из Go) — в git-истории и README.

### 1. Свой формат запросов `*.routy` — приоритет
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
- имена уникальны в проекте без импортов, при конфликте — с папкой (`users.Create()`); циклы и конфликты — ошибка `routy check`
- упавший `expect` вызванного запроса роняет вызывающий, ошибка с цепочкой: `CreateOrder → Login: status == 200 — got 401`
- `flow Name { … }` — сценарий; `routy run Checkout`, кнопка в GUI; вкладка «Trace» с вызванными запросами

Задачи:
- [ ] Спека формата в `docs/` (EN + RU): грамматика, выражения и встроенные функции, экранирование, кеш вызовов, `routy fmt`
- [ ] Парсер и вычислитель выражений в core (замена `parser.rs` / `expr.rs`), ошибки с позицией, `routy check`
- [ ] Вызовы запросов, кеш, проверка циклов, `flow`, trace в CLI (`-v`) и GUI
- [ ] `routy fmt` — один канонический вид (как `gofmt`)
- [ ] `.http` — режим совместимости (читаем как раньше) + `routy convert api/` в `*.routy`
- [ ] Генерация `*.routy` в `import go`: имя и описание из doc-комментария, `params`, тело из структуры
- [ ] GUI: подсветка и автодополнение нового формата в CodeMirror, вкладка Trace
- [ ] `examples/api`, README, docs, `routy init` — на новый формат

### 2. Коллекция, которая не устаревает (киллер-фича)
Питч: «одна строка в CI — и API-тесты больше не разъедутся с кодом». Bruno/Postman о коде ничего не знают.
- [ ] `routy import go --check`: exit code ≠ 0, если в коде есть роут без запроса или запрос ссылается на удалённый роут
  (сравнение «новые / пропавшие» уже есть в `import/mod.rs`)
- [ ] Дрейф тела: `required`-поле Go-структуры, которого нет в `body`, — ошибка `--check`
- [ ] `body matches Order` по Go-структуре из кода (теги `json` уже разбираем); импорт генерирует его из типа ответа
  (`json.Encode` / `c.JSON`)
- [ ] JUnit-отчёт (`--report junit.xml`)
- [ ] Рецепт для CI в docs (GitHub Actions, GitLab) и готовый `routy-action`

### 3. Редакторы без GUI: `routy lsp`
- [ ] `routy lsp` (stdio, `tower-lsp` или `lsp-server`) внутри того же бинаря
- [ ] Диагностика как `routy check`; автодополнение переменных (`Vars::list`), встроенных функций и имён запросов
- [ ] Hover: значение и источник переменной (`Vars::lookup`, секреты замаскированы), сигнатура вызываемого запроса
- [ ] Go to definition: переменная → строка в `env.toml`, `Login()` → его файл, `Order` → Go-структура
- [ ] Code lens «Send» / «Run flow» → ответ в отдельном буфере
- [ ] Грамматика tree-sitter для `*.routy` (подсветка в Neovim, Helix, Zed)
- [ ] Страница в docs: настройка редакторов

### 4. VS Code-расширение
LSP-клиент + интерфейс; логика вся в `routy`, своих правил в расширении нет.
- [ ] Платформенные VSIX с бинарём `routy` внутри; Marketplace + Open VSX (Cursor, Windsurf)
- [ ] Code lens «▶ Send», «Send in prod…», «Copy as curl»; ответ в webview (компоненты из `app/src`: ResponseView, BodyViewer)
- [ ] Боковая панель: дерево запросов, env, переменные, Routes с синхронизацией, история, Trace
- [ ] Общие React-компоненты вынести в пакет для app и расширения

### 5. MCP для AI-агентов
- [ ] `routy mcp` (stdio): «список запросов», «выполнить запрос/flow в env», «показать переменные» поверх `Runner`/`Vars`
- [ ] Секреты подставляются внутри routy и не попадают к агенту (значения из keyring/`secrets` маскируются)
- [ ] Страница в docs: подключение к Claude Code / Cursor

### 6. Распространение
- [ ] install.sh и `routy update` для macOS (архивы уже есть) и Windows (`install.ps1`, zip)
- [ ] Пакетные менеджеры для CLI: Homebrew tap, AUR, COPR, Scoop/winget
- [ ] Подпись и нотаризация macOS, подпись Windows-установщика

### Потом
- Импорт роутов из других языков (FastAPI, Express, Spring) — когда Go-история заработает
- Импорт из Postman / Insomnia / OpenAPI в `*.routy`
- GraphQL, WebSocket — не раньше этапов 1–4

## Релизы и автообновление

**Один раз:**
1. Ключ подписи обновлений уже сгенерирован в `~/.tauri/routy.key` (публичный ключ — в `app/src-tauri/tauri.conf.json`).
   Сделайте резервную копию: если ключ потерять, установленные приложения больше не примут обновление.
   Лучше перегенерировать с паролем до первого релиза: `cargo tauri signer generate -w ~/.tauri/routy.key -f`
   и обновить `plugins.updater.pubkey`.
2. GitHub → Settings → Secrets → Actions:
   `TAURI_SIGNING_PRIVATE_KEY` = содержимое `~/.tauri/routy.key`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` = пароль (или пусто).
3. Если репозиторий не `github.com/1rowvy/routy` — поправить `endpoints` в `tauri.conf.json` и `repository` в `Cargo.toml`.

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
(`xattr -dr com.apple.quarantine /Applications/Routy.app`). Секреты для подписи перечислены в шапке `release.yml`.
