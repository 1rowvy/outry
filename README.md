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

```routy
// api/users/create.routy

// Create user
POST /users {
  headers { Authorization: "Bearer ${Login().body.token}" }
  body { name: "Viktor", role: "admin" }

  expect {
    status == 201
    body.name == "Viktor"
    body matches { id: string, name: string }
  }

  save user_id = body.id
}
```

```console
$ routy run api
✓ api/auth/login.routy  Login  POST http://localhost:8080/login  200 OK  12ms 184B
    ✓ status == 200
✓ api/users/create.routy  CreateUser  POST http://localhost:8080/users  201 Created  9ms 61B
    ↳ Login  cached
    ✓ status == 201
    ✓ body.name == "Viktor"
    ✓ body matches { id: string, name: string }
    → saved user_id
✗ api/users/get.routy  GetUser  GET http://localhost:8080/users/7  200 OK  4ms 211B
    ✗ body.role == "admin"  — body.role is "user"

2 passed, 1 failed
```

## Why Routy

- **Requests live in git.** `.routy` files: a JSON body, checks that read like code, no Go or JS needed. Review them in pull requests, grep them, edit them in any editor. The app picks up outside changes instantly.
- **The same engine everywhere.** The desktop app and the `routy` CLI call the same Rust core, so a request that works in the GUI works in CI. They can't drift apart.
- **Secrets stay out of the repo.** Tokens go to the system keychain locally and come from `ROUTY_*` environment variables in CI, so there's nothing to commit by accident.
- **Requests call requests.** `Login().body.token` logs in once per run and reuses the response; no run order to maintain. `flow` describes a scenario; `expect` checks status, timing, headers and the body's shape. A failure gives a non-zero exit code.
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
routy init                 # creates api/env.toml and api/health.routy
routy run api              # sends every request in api/, alphabetically
routy run CreateUser       # or one, by name
```

Or open the folder in the desktop app. It offers to create `api/env.toml` if there isn't one.

Have a Go service? Generate a request for every route (chi, gin, `net/http`); existing files are left alone:

```sh
routy import go .          # + api/users/get-by-id.routy  GET /users/{id}
```

See [Import routes from Go](https://1rowvy.github.io/routy/guides/import-go/).

## Request files

The smallest file is one line — `GET /health`. Everything else is added when needed:

```routy
// Get order
// The comment's first line is the name: GetOrder.
GET /orders/{id} {
  query { expand: "items" }
  headers { X-Request-Id: uuid() }

  expect {
    status == 200
    body.items.length > 0
    body.items.all(i => i.qty > 0)
    body matches Order
  }
}

shape Order {
  id: string,
  total: number,
  items: [{ sku: string, qty: integer }],
}

// Checkout
flow Checkout {
  order = CreateOrder(shop: "main")
  Pay(order: order.body.id)
  expect { GetOrder(id: order.body.id).body.status == "paid" }
}
```

| Part | Rule |
|------|------|
| Request | `METHOD /path` is appended to `base`; `https://…` is used as is. `{id}` in the path is a parameter |
| Name | The first line of the `//` comment above: `// Get order` → `GetOrder` |
| Fields | `params`, `only: [dev]`, `confirm: true`, `timeout: 10s`, `cache: 30m`, `query`, `headers`, `body` / `form` / `multipart`, `poll`, `expect`, `save` |
| Body | JSON5 with expressions: `{ name, role: "admin", id: CreateUser().body.id }` |
| Values | Bare names are variables; `"${name}"` in strings; `uuid()`, `now()`, `randomInt(1, 10)` |
| Checks | `== != < > && \|\| !`, `in`, `matches /re/`, `matches Shape`, `matches schema("x.json")`, `.contains()`, `.length`, … |
| Calls | `Login(email: "a")` is the response of `Login`; sent once per run, `fresh Login()` sends again |

Full reference: [.routy format](https://1rowvy.github.io/routy/reference/routy-format/).
`routy fmt` keeps files in one style; `routy check --env prod` finds missing variables and calls
blocked by `only` without sending anything.

Older `.http` files keep working next to `.routy`, and `routy convert api/` rewrites them —
see [.http files](https://1rowvy.github.io/routy/guides/request-format/).

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

When Routy resolves a variable, the first match wins:

1. `--var name=value` on the command line
2. values captured by `save`
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

- name: Lint request files
  run: |
    routy fmt --check api
    routy check api --env ci --no-keyring

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
routy run <PATHS|NAMES>...             # send requests; folders run alphabetically, names and file:line too
    -e, --env <ENV>                    #   environment from env.toml
    --var <NAME=VALUE>                 #   override a variable (repeatable)
    --fail-fast                        #   stop at the first failure
    -v, --verbose                      #   print response headers and body
    --json                             #   JSON Lines output for CI
    --fresh                            #   ignore values saved by previous runs
    --no-keyring                       #   secrets only from ROUTY_*
    --yes                              #   send `confirm: true` requests without asking
routy check <PATHS>... [--env ENV]     # syntax, names, arguments, shapes, cycles; with --env also
                                       #   `only` and missing variables. Nothing is sent
routy fmt [PATHS]... [--check]         # canonical style for *.routy, like gofmt
routy convert [PATHS]... [--rm]        # *.http → *.routy
routy vars [-e ENV] [--reveal]         # final variable values and their sources
routy envs                             # list environments (* = default)
routy import go [DIR] [--dry-run]      # create requests for Go routes that have no file yet
routy secret set|rm <NAME> [-e ENV]    # manage keychain secrets
routy update [--check]                 # self-update from GitHub releases
```

All commands and flags: [CLI reference](https://1rowvy.github.io/routy/reference/cli/).

## Desktop app

- A file tree of every `*.routy` and `*.http` in the project with rename / move / delete, plus an environment switcher
- An editor with highlighting, live `routy check` and completion of requests, variables and functions.
  <kbd>Ctrl</kbd>+<kbd>Enter</kbd> (or ▶ next to a request) runs the request or flow at the cursor, <kbd>Ctrl</kbd>+<kbd>S</kbd> saves
- A trace of the requests a request or flow called, with what came from the run's cache
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
crates/routy-core   .routy parser, checker, formatter and executor; .http support, variables, keychain,
                    Go route import: all behavior lives here
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
