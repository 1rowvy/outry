# Routy

API-клиент, где запросы — текстовые файлы в репозитории. Desktop-приложение (Tauri) и CLI для CI
работают на одном ядре, поэтому ведут себя одинаково.

📖 Документация: **https://1rowvy.github.io/routy/** ([на русском](https://1rowvy.github.io/routy/ru/))

## Установка

### CLI (Linux)

```sh
curl -fsSL https://raw.githubusercontent.com/1rowvy/routy/master/install.sh | sh
```

Скрипт скачивает статический бинарь из последнего релиза (x86_64 или aarch64, работает на любом дистрибутиве),
проверяет sha256 и кладёт его в `~/.local/bin/routy`. Настройки через переменные окружения:

```sh
# конкретная версия
curl -fsSL https://raw.githubusercontent.com/1rowvy/routy/master/install.sh | ROUTY_VERSION=v0.2.0 sh

# для всех пользователей (обновлять потом тоже через sudo routy update)
curl -fsSL https://raw.githubusercontent.com/1rowvy/routy/master/install.sh | sudo ROUTY_INSTALL_DIR=/usr/local/bin sh
```

Обновление и удаление:

```sh
routy update            # скачать последнюю версию (с проверкой sha256)
routy update --check    # только узнать, есть ли новая
rm ~/.local/bin/routy   # удалить
```

Как npm, routy раз в сутки в фоне проверяет новую версию и после команды напоминает об обновлении
(команды при этом не ждут сети). Напоминание не показывается в пайпах и CI; отключить — `ROUTY_NO_UPDATE_NOTIFIER=1`.

macOS и Windows: архивы `routy-cli-*` в [релизах](https://github.com/1rowvy/routy/releases/latest).

### Desktop-приложение

Установщики для macOS, Windows и Linux (AppImage, `.deb`, `.rpm`) — в [релизах](https://github.com/1rowvy/routy/releases/latest).
Приложение обновляется само: при старте проверяет новую версию и предлагает установить.
На Linux самообновление работает только у AppImage.

## Формат

`api/users/create.http`:

```http
# комментарий
POST {{base}}/users
Authorization: Bearer {{token}}

{"name": "Viktor"}

> save user_id = body.id
> assert status == 201
> assert body.name == "Viktor"
```

- первая строка — `МЕТОД URL` (метод можно опустить — будет GET);
- дальше заголовки, пустая строка, тело;
- строки `>` в конце файла — директивы:
  - `> save <имя> = <путь>` — сохранить значение из ответа для следующих запросов;
  - `> assert <путь> <op> <значение>` — проверка; op: `==` `!=` `<` `<=` `>` `>=` `contains` `exists`.
- пути: `status`, `duration`, `body`, `body.items[0].id`, `body["weird key"]`, `headers.content-type`.

`api/env.toml`:

```toml
default = "dev"

[vars]            # общие для всех окружений
version = "v1"

[env.dev]
base = "http://localhost:8080"

[env.prod]
base = "https://api.example.com"
```

Секреты в файлы не пишутся — они в системном хранилище паролей:

```sh
routy secret set token --env dev     # значение из stdin
```

В CI вместо хранилища — переменные окружения `ROUTY_<ИМЯ>` (`ROUTY_TOKEN=...`).

## CLI

```sh
routy init                          # создать api/env.toml и пример
routy run api/users/create.http     # один запрос
routy run api --env prod            # все *.http по алфавиту, цепочкой
routy run api --json --fail-fast    # для CI; exit code 1, если что-то упало
routy check api                     # только синтаксис
routy envs
```

## Разработка

```sh
cargo test -p routy-core -p routy-cli
cargo run -p routy-cli -- run examples/api

cd app && npm install
npm run tauri dev
```

Linux: нужны `webkit2gtk-4.1`, `libdbus-1` (см. `.github/workflows/ci.yml`).

План, релизы и автообновление — в [PLAN.md](PLAN.md).
