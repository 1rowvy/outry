# Routy — план

API-клиент, где запросы — текстовые файлы в репозитории, а коллекция собирается сама из кода.
Аналог Insomnia/Postman, но без облака и без экспорта/импорта коллекций: правда — в `api/*.http`.

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
2. значения из `> save` (сохраняются между запусками в `~/.local/share/routy/state/`, не в репо)
3. переменные процесса `ROUTY_<NAME>` — секреты в CI
4. `[env.<name>]` из `env.toml`, затем общие `[vars]`
5. системное хранилище паролей (`keyring`): `routy secret set token --env dev`

## Этапы

### ✅ 0. Каркас и инфраструктура
- [x] Cargo workspace, единая версия в `[workspace.package]`
- [x] CI: fmt, clippy, тесты core/cli на Linux/macOS/Windows; typecheck + clippy приложения
- [x] Release: тег `vX.Y.Z` → приложение (macOS arm64/x64, Linux, Windows) + CLI (5 таргетов) → публикация
- [x] Автообновление: `tauri-plugin-updater`, подписанные артефакты, `latest.json` в GitHub Releases
- [x] `scripts/release.sh X.Y.Z` — поднять версию везде

### ✅ 1. core: формат + отправка
- [x] Парсер `.http`: метод (опционален), URL, заголовки, тело, комментарии `#` `//`, CRLF, ошибки с номером строки
- [x] `{{var}}`, ошибка сразу со списком всех недостающих переменных
- [x] `env.toml` с окружениями и общими `[vars]`, поиск проекта вверх по дереву
- [x] Секреты в keyring, мягкая деградация без D-Bus (headless CI)
- [x] `> save name = body.path`, `> assert <path> <op> <value>` (`== != < <= > >= contains exists`)
- [x] Авто-`Content-Type: application/json` для JSON-тела
- [x] Юнит-тесты + сквозной тест с локальным HTTP-сервером

### ✅ 2. CLI
- [x] `routy run <файлы|каталоги>` — цепочки по порядку, `--env`, `--var`, `--fail-fast`, `-v`, `--json`, `--fresh`, exit code
- [x] `routy check`, `routy envs`, `routy secret set|rm`, `routy init`
- [x] Установка `curl … install.sh | sh` (Linux, статическая musl-сборка, проверка sha256)
- [x] `routy update` / `routy update --check` — самообновление из GitHub Releases с проверкой sha256
- [x] Напоминание о новой версии после команд (раз в сутки, фоном, как у npm)
- [ ] install.sh и `routy update` для macOS (архивы уже есть) и Windows (`install.ps1`, zip)

### ✅ 3. Desktop (Tauri + React)
- [x] Открытие проекта, дерево файлов, редактор, отправка (Ctrl+Enter), сохранение (Ctrl+S)
- [x] Вкладки ответа: body (pretty JSON) / headers / tests / request
- [x] Проверка синтаксиса на лету, live-reload при правке файлов снаружи (`notify`)
- [x] Выбор окружения, ввод секретов в keyring
- [x] Каталог без `env.toml` → кнопка «Создать api/env.toml» (та же `project::init`, что и `routy init`)
- [x] Баннер обновления: скачать → установить → перезапуск
- [x] CodeMirror 6 вместо textarea: подсветка `.http`/TOML, подчёркивание ошибки на строке, автодополнение `{{var}}` и директив
- [x] Переименование / удаление / перемещение файлов, контекстное меню дерева
- [x] История ответов (в памяти + опционально на диск вне репо)
- [x] Отмена запроса, параллельные запросы (сессия блокируется только на подстановку и разбор ответа)
- [x] Подсветка JSON, поиск по ответу, превью картинок/HTML, сохранение тела в файл
- [x] Меню «Проверить обновления», настройка «обновляться автоматически»

### ✅ 4. Переменные и окружения
- [x] Панель переменных: что откуда пришло (env / saved / secret / ROUTY_*), очистка saved
- [x] Динамические переменные: `{{$uuid}}`, `{{$timestamp}}`, `{{$randomInt}}`, `{{$randomInt min max}}`
- [x] `routy vars` в CLI — показать итоговые значения (секреты замаскированы, `--reveal`)
- [x] Объявление секретов в `env.toml` (`secrets = ["token"]`), чтобы GUI подсказывал, чего не хватает

### 5. Импорт роутов из Go
- [ ] `routy-core::import` на `tree-sitter` + `tree-sitter-go`
- [ ] chi: `r.Get/Post/...("/path", h)`, `r.Route("/prefix", func(r chi.Router){...})`, `r.Mount`
- [ ] gin: `r.GET(...)`, `r.Group("/v1")` с учётом префиксов групп
- [ ] Роутеры как шаблоны-запросы tree-sitter (`queries/chi.scm`, `queries/gin.scm`) — добавление роутера без кода на Rust
- [ ] `/users/{id}` и `/users/:id` → `{{id}}`
- [ ] Генерация `api/<resource>/<method>.http` только для отсутствующих файлов; `--dry-run`, отчёт «новые / пропавшие роуты»
- [ ] `routy import go ./cmd/server`, кнопка «Синхронизировать» в GUI
- [ ] Позже: тело запроса из структур хендлеров (`json.Decode(&req)` → поля структуры с тегами `json:`)

### 6. Цепочки и проверки
- [ ] `routy run` по сценарию: `api/flows/signup.flow` со списком файлов
- [ ] Ещё операторы: `matches /regex/`, `in [..]`, `type == array`, `length`
- [ ] JUnit-отчёт (`--report junit.xml`) для CI
- [ ] Запуск коллекции в GUI с отчётом

### Потом
- Несколько запросов в одном файле через `###` (совместимость с JetBrains HTTP Client / VS Code REST Client)
- Импорт из Postman / Insomnia / OpenAPI
- GraphQL, WebSocket, multipart, файлы в теле (`< ./payload.json`)
- Подпись и нотаризация macOS, подпись Windows-установщика
- Пакетные менеджеры для CLI: Homebrew tap, AUR, COPR, Scoop/winget

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
