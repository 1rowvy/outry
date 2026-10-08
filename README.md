# Routy

API-клиент, где запросы — текстовые файлы в репозитории. Desktop-приложение (Tauri) и CLI для CI
работают на одном ядре, поэтому ведут себя одинаково.

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
