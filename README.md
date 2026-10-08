<div align="center">

<img src="app/app-icon.svg" width="96" alt="Routy logo">

# Routy

**An API client where every request is a plain text file in your repo.**<br>
A desktop app for clicking through requests and a CLI for running them as tests in CI — on one shared core.

[![CI](https://github.com/1rowvy/routy/actions/workflows/ci.yml/badge.svg)](https://github.com/1rowvy/routy/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/1rowvy/routy)](https://github.com/1rowvy/routy/releases/latest)
[![Docs](https://img.shields.io/badge/docs-1rowvy.github.io%2Frouty-blue)](https://1rowvy.github.io/routy/)
[![License: MIT](https://img.shields.io/badge/license-MIT-green)](LICENSE)

[Documentation](https://1rowvy.github.io/routy/) · [Getting started](https://1rowvy.github.io/routy/getting-started/) · [Releases](https://github.com/1rowvy/routy/releases/latest) · [На русском](https://1rowvy.github.io/routy/ru/)

</div>

---

```http
# api/users/create.http
POST {{base}}/users
Authorization: Bearer {{token}}

{"name": "Viktor"}

> save user_id = body.id
> assert status == 201
> assert body.name == "Viktor"
```

```console
$ routy run api
✓ api/auth/login.http  POST http://localhost:8080/login  200 OK  12ms 184B
    ✓ status == 200
    → saved token
✓ api/users/create.http  POST http://localhost:8080/users  201 Created  9ms 61B
    ✓ status == 201
    ✓ body.name == "Viktor"
    → saved user_id
✗ api/users/get.http  GET http://localhost:8080/users/7  200 OK  4ms 211B
    ✗ status == 201  (actual: 200)

2 passed, 1 failed
```

## Why Routy

- **Requests live in git.** One `.http` file per request. Review them in pull requests, grep them, edit them in any editor. The app picks up outside changes instantly.
- **The same engine everywhere.** The desktop app and the `routy` CLI call the same Rust core, so a request that works in the GUI works in CI. They can't drift apart.
- **Secrets stay out of the repo.** Tokens go to the system keychain locally and come from `ROUTY_*` environment variables in CI, so there's nothing to commit by accident.
- **Chains and checks built in.** `> save` a value from one response and use it in the next request. `> assert` on status, timing, headers and the JSON body. A failure gives a non-zero exit code.
- **No account, no cloud, no lock-in.** It's plain text: if you stop using Routy, your requests are still readable files.

## Installation

### CLI

Linux (x86_64 or aarch64, static binary, any distro):

```sh
curl -fsSL https://raw.githubusercontent.com/1rowvy/routy/master/install.sh | sh
```

The script downloads the latest release, verifies its SHA-256 and installs it to `~/.local/bin/routy`.

<details>
<summary>Options, updating, uninstalling</summary>

```sh
# a specific version
curl -fsSL https://raw.githubusercontent.com/1rowvy/routy/master/install.sh | ROUTY_VERSION=v0.2.0 sh

# system-wide (update with `sudo routy update` later)
curl -fsSL https://raw.githubusercontent.com/1rowvy/routy/master/install.sh | sudo ROUTY_INSTALL_DIR=/usr/local/bin sh
```

```sh
routy update            # install the latest version (SHA-256 verified)
routy update --check    # only check whether a newer one exists
rm ~/.local/bin/routy   # uninstall
```

Like npm, routy checks for a new version in the background once a day and shows a short notice after a
command. Commands never wait for the network. The notice is hidden in pipes and CI; turn it off with
`ROUTY_NO_UPDATE_NOTIFIER=1`.

</details>

macOS and Windows: download a `routy-cli-*` archive from [releases](https://github.com/1rowvy/routy/releases/latest).

### Desktop app

Installers for **macOS**, **Windows** and **Linux** (AppImage, `.deb`, `.rpm`) are on the
[releases page](https://github.com/1rowvy/routy/releases/latest). The app checks for updates on startup
and installs them in one click (on Linux, only the AppImage self-updates).

## Quick start

```sh
cd my-service
routy init                 # creates api/env.toml and api/health/get.http
routy run api              # sends every request in api/, alphabetically
```

Or open the folder in the desktop app. It offers to create `api/env.toml` if there isn't one.

Have a Go service? Generate a request for every route (chi, gin, `net/http`); existing files are left alone:

```sh
routy import go .          # + api/users/get-by-id.http   GET /users/{{id}}
```

See [Import routes from Go](https://1rowvy.github.io/routy/guides/import-go/).

## Request files

```http
# Comments start with # or //
POST {{base}}/users?version={{version}}
Content-Type: application/json
Authorization: Bearer {{token}}

{"name": "Viktor", "role": "admin"}

> save user_id = body.id
> assert status == 201
> assert duration < 500
> assert headers.content-type contains json
```

| Part | Rule |
|------|------|
| Request line | `METHOD URL`. The method is optional and defaults to `GET` |
| Headers | `Name: value`, one per line, until the first blank line |
| Body | Everything after the blank line. JSON bodies get `Content-Type: application/json` automatically |
| Variables | `{{name}}` anywhere: URL, headers, body. `{{$uuid}}`, `{{$timestamp}}`, `{{$randomInt 1 100}}` are generated per request |
| Directives | Lines starting with `>` at the end of the file, run after the response arrives |

**Directives**

- `> save <name> = <path>` stores a value for the following requests (and later runs).
- `> assert <path> <op> <value>` checks the response. Operators: `==` `!=` `<` `<=` `>` `>=` `contains` `exists`.

**Paths:** `status`, `duration`, `body`, `body.items[0].id`, `body["weird key"]`, `headers.content-type`.

Full reference: [request format](https://1rowvy.github.io/routy/guides/request-format/) ·
[expressions](https://1rowvy.github.io/routy/reference/expressions/).

## Environments and variables

```toml
# api/env.toml
default = "dev"            # used when --env is not given

[vars]                     # shared by all environments
version = "v1"

[env.dev]
base = "http://localhost:8080"

[env.prod]
base = "https://api.example.com"
version = "v2"             # overrides [vars]
```

When Routy resolves `{{name}}`, the first match wins:

1. `--var name=value` on the command line
2. values captured by `> save`
3. `ROUTY_<NAME>` environment variables
4. `[env.<current>]`, then `[vars]` in `env.toml`
5. the system keychain

If any variable is missing, the request is not sent, and every missing name is reported at once.
`routy vars` shows the final value of every variable and where it came from (secrets masked).

### Secrets

Secrets never go into files. Store them in the OS keychain (macOS Keychain, Windows Credential Manager,
Secret Service on Linux):

```sh
routy secret set token --env dev     # value is read from stdin
```

List secret names in `env.toml` (`secrets = ["token"]`) and `routy vars` and the app will point out the
ones that are missing.

In the app, use **Add secret** at the bottom of the sidebar. In CI, set `ROUTY_TOKEN=...` instead.

## Running in CI

```yaml
# .github/workflows/api-tests.yml
- name: Install routy
  run: |
    curl -fsSL https://raw.githubusercontent.com/1rowvy/routy/master/install.sh | sh
    echo "$HOME/.local/bin" >> "$GITHUB_PATH"

- name: Run API tests
  run: routy run api --env ci --no-keyring --fail-fast
  env:
    ROUTY_TOKEN: ${{ secrets.API_TOKEN }}
```

Exit codes: `0` everything passed, `1` at least one request failed, `2` Routy could not start.
Use `--json` for JSON Lines output, one object per request. See the [CI guide](https://1rowvy.github.io/routy/guides/ci/).

## CLI reference

```sh
routy init [DIR]                       # create api/env.toml and an example request
routy run <PATHS>...                   # send requests; folders run alphabetically as a chain
    -e, --env <ENV>                    #   environment from env.toml
    --var <NAME=VALUE>                 #   override a variable (repeatable)
    --fail-fast                        #   stop at the first failure
    -v, --verbose                      #   print response headers and body
    --json                             #   JSON Lines output for CI
    --fresh                            #   ignore values saved by previous runs
    --no-keyring                       #   secrets only from ROUTY_*
routy check <PATHS>...                 # syntax check, nothing is sent
routy vars [-e ENV] [--reveal]         # final variable values and their sources
routy envs                             # list environments (* = default)
routy import go [DIR] [--dry-run]      # create requests for Go routes that have no file yet
routy secret set|rm <NAME> [-e ENV]    # manage keychain secrets
routy update [--check]                 # self-update from GitHub releases
```

All commands and flags: [CLI reference](https://1rowvy.github.io/routy/reference/cli/).

## Desktop app

- A file tree of every `*.http` in the project with rename / move / delete, plus an environment switcher
- An editor with live syntax checking. <kbd>Ctrl</kbd>+<kbd>Enter</kbd> sends, <kbd>Ctrl</kbd>+<kbd>S</kbd> saves
- Requests run in parallel and can be cancelled
- The response shows highlighted, searchable body, image and HTML previews, headers, test results and the
  exact request that was sent; the body can be saved to a file
- Response history (in memory, or on disk outside the repo) and a variables panel showing where every value comes from
- A built-in `env.toml` editor with TOML validation
- Routes tab: syncs request files with the routes of a Go service
- Changes made outside the app (VS Code, `git pull`) show up immediately

More in the [desktop app guide](https://1rowvy.github.io/routy/guides/desktop-app/).

## Development

```
crates/routy-core   parser, templating, runner, assertions, keychain: all behavior lives here
crates/routy-cli    the `routy` binary, a thin wrapper over core
app/                Tauri 2 + React desktop app, also a thin wrapper over core
docs/               Astro Starlight documentation site (English + Russian)
examples/api        sample requests against httpbin.org
```

```sh
cargo test -p routy-core -p routy-cli
cargo run -p routy-cli -- run examples/api --fresh --no-keyring

cd app && npm install && npm run tauri dev
```

The desktop app on Linux needs `webkit2gtk-4.1` (see [`.github/workflows/ci.yml`](.github/workflows/ci.yml)).
The roadmap and the release process are in [PLAN.md](PLAN.md).

## License

[MIT](LICENSE)
