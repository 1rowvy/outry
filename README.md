<div align="center">

<img src="app/app-icon.svg" width="96" alt="Outry logo">

# Outry

**Executable API specs that live in your repository.**<br>
Describe every endpoint of your service in plain `.outry` files next to the code. Run them from the terminal,
your editor, the desktop app or CI — and when the code changes, Outry shows which requests no longer match it.

[![CI](https://github.com/1rowvy/outry/actions/workflows/ci.yml/badge.svg)](https://github.com/1rowvy/outry/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/1rowvy/outry)](https://github.com/1rowvy/outry/releases/latest)
[![Docs](https://img.shields.io/badge/docs-1rowvy.github.io%2Foutry-blue)](https://1rowvy.github.io/outry/)
[![License: MIT](https://img.shields.io/badge/license-MIT-green)](LICENSE)

[Documentation](https://1rowvy.github.io/outry/) · [Getting started](https://1rowvy.github.io/outry/getting-started/) · [Example project](https://github.com/1rowvy/outry-example-gin) · [Releases](https://github.com/1rowvy/outry/releases/latest) · [На русском](https://1rowvy.github.io/outry/ru/)

</div>

---

### 1. Describe

A request is a small, readable block: where it goes, what it sends, what the answer must look like. Requests
call each other like functions, so a scenario is just code.

```outry
// api/orders/create.outry

CreateOrder: POST /v1/orders {
  headers { Authorization: "Bearer ${Login().body.token}" }
  body { user_id: CreateUser().body.id, items: [{ sku: "BOOK-1", qty: 2 }] }

  expect {
    status == 201
    body matches Order
    body.total == 25
  }

  save order_id = body.id
}
```

### 2. Run anywhere

The same files run from the CLI, the desktop app, VS Code or any LSP editor — one Rust core behind all of them.

```console
$ outry run api
✓ api/auth/login.outry  Login  POST http://localhost:8080/auth/login  200 OK  1ms 62B
    ✓ status == 200
    ✓ body matches Token
✓ api/flows/checkout.outry  Checkout  flow
    ↳ CreateUser  201  0ms
      ↳ Login  cached
    ↳ CreateOrder  201  0ms
    ↳ PayOrder  202  0ms
    ↳ WaitUntilPaid  200  1ms
    ✓ paid.body.total == 45.5
✗ api/orders/get.outry  GetOrder  GET http://localhost:8080/v1/orders/7  200 OK  0ms 211B
    ✗ body.status == "paid"  — body.status is "pending"

21 passed, 1 failed
```

### 3. Stay in sync with the code

Outry reads your Go service — chi, gin or `net/http` — and knows which handler each request belongs to: the
struct the body is bound into, query parameters, headers, middleware and the type it responds with. Rename a
field in Go, and the pull request says which requests are now wrong:

```console
$ outry check
api/v1/users/post.outry:17:8: error: required field `full_name` (string) is missing from body  ← internal/api/users.go:13
api/v1/users/post.outry:17:10: error: body field `name` is not in model.CreateUser  ← internal/api/users.go:13
14 files, 2 environments, 12 Go routes: 2 errors, 0 warnings; 2 fixable with `outry import go --fix`
```

`outry import go .` writes a request for every new route; `--fix` updates the existing ones.

**See it on a real service:** [outry-example-gin](https://github.com/1rowvy/outry-example-gin) — a Gin shop API
whose `api/` folder uses every feature of Outry. Clone it, `go run .`, `outry run api`, then change the code.

## Why Outry

- **The spec is the test.** A `.outry` file documents an endpoint, sends it, checks the answer and serves as a
  step of bigger scenarios. One artifact instead of a wiki page, a collection and a test suite that disagree.
- **It lives with the code.** Plain text in your repository: reviewed in pull requests, versioned with the
  service, readable without Outry installed. No account, no cloud, no export.
- **It notices when the code moves.** `outry check` compares every request with its Go handler and fails CI
  on drift, with annotations on the exact line.
- **Requests compose like functions.** `Login().body.token` logs in once per run and reuses the answer;
  `flow Checkout { … }` is a scenario; `poll` waits for async work; shapes describe responses once.
- **Secrets stay out of the repo.** The system keychain locally, `OUTRY_*` variables in CI.
- **One engine everywhere.** The CLI, the desktop app, the VS Code extension and `outry lsp` share the same
  core — what works on your machine works in CI.

## Installation

### CLI

Linux (x86_64 or aarch64, static binary, any distro):

```sh
curl -fsSL https://raw.githubusercontent.com/1rowvy/outry/master/install.sh | sh
```

The script downloads the latest release, verifies its SHA-256 and installs it to `~/.local/bin/outry`,
along with tab completion for fish and bash (zsh: `source <(outry completions zsh)` in `~/.zshrc`).

<details>
<summary>Options, updating, uninstalling</summary>

```sh
# a specific version
curl -fsSL https://raw.githubusercontent.com/1rowvy/outry/master/install.sh | OUTRY_VERSION=v0.2.0 sh

# system-wide (update with `sudo outry update` later)
curl -fsSL https://raw.githubusercontent.com/1rowvy/outry/master/install.sh | sudo OUTRY_INSTALL_DIR=/usr/local/bin sh
```

```sh
outry update            # install the latest version (SHA-256 verified)
outry update --check    # only check whether a newer one exists
# uninstall: the binary, tab completion, saved values and caches
rm ~/.local/bin/outry ~/.config/fish/completions/outry.fish ~/.local/share/bash-completion/completions/outry
rm -rf ~/.local/share/outry ~/.cache/outry
```

Like npm, outry checks for a new version in the background once a day and shows a short notice after a
command. Commands never wait for the network. The notice is hidden in pipes and CI; turn it off with
`OUTRY_NO_UPDATE_NOTIFIER=1`.

</details>

macOS and Windows: download a `outry-cli-*` archive from [releases](https://github.com/1rowvy/outry/releases/latest).

### VS Code

[Outry for VS Code](https://marketplace.visualstudio.com/items?itemName=outry.outry-vscode) (also on
[Open VSX](https://open-vsx.org/extension/outry/outry-vscode) for Cursor, Windsurf, VSCodium) bundles the CLI.

### Desktop app

Installers for **macOS**, **Windows** and **Linux** (AppImage, `.deb`, `.rpm`) are on the
[releases page](https://github.com/1rowvy/outry/releases/latest). The app checks for updates on startup
and installs them in one click (on Linux, only the AppImage self-updates).

## Quick start

```sh
cd my-service
outry init                 # api/env.toml with environments and api/health.outry
outry import go .          # Go service? a request for every route and a shape for every response
outry run api              # send every request in api/
outry run CreateUser       # or one, by name
outry check                # nothing is sent: syntax, environments, formatting, drift from the code
```

Or open the folder in the desktop app or VS Code. More: [Getting started](https://1rowvy.github.io/outry/getting-started/),
[Import routes from Go](https://1rowvy.github.io/outry/guides/import-go/).

## Request files

The smallest file is one line — `GET /health`. Everything else is added when needed:

```outry
// The name comes before the method; the comment is the description.
GetOrder: GET /orders/{id} {
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
| Name | Before the method: `GetOrder: GET /orders/{id}`; the `//` comment above is the description |
| Fields | `params`, `only: [dev]`, `confirm: true`, `timeout: 10s`, `cache: 30m`, `query`, `headers`, `body` / `form` / `multipart`, `poll`, `expect`, `save` |
| Body | JSON5 with expressions: `{ name, role: "admin", id: CreateUser().body.id }` |
| Values | Bare names are variables; `"${name}"` in strings; `uuid()`, `now()`, `randomInt(1, 10)` |
| Checks | `== != < > && \|\| !`, `in`, `matches /re/`, `matches Shape`, `matches schema("x.json")`, `.contains()`, `.length`, … |
| Calls | `Login(email: "a")` is the response of `Login`; sent once per run, `fresh Login()` sends again |

Full reference: [.outry format](https://1rowvy.github.io/outry/reference/outry-format/).
`outry fmt` keeps files in one style; `outry check --env prod` finds missing variables and calls
blocked by `only` without sending anything.

Older `.http` files keep working next to `.outry`, and `outry convert api/` rewrites them —
see [.http files](https://1rowvy.github.io/outry/guides/request-format/).

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

When Outry resolves a variable, the first match wins:

1. `--var name=value` on the command line
2. values captured by `save`
3. `OUTRY_<NAME>` environment variables
4. `[env.<current>]`, then `[vars]` in `env.toml`
5. the system keychain

If any variable is missing, the request is not sent, and every missing name is reported at once.
`outry vars` shows the final value of every variable and where it came from (secrets masked).

### Secrets

Secrets never go into files. Store them in the OS keychain (macOS Keychain, Windows Credential Manager,
Secret Service on Linux):

```sh
outry secret set token --env dev     # value is read from stdin
```

List secret names in `env.toml` (`secrets = ["token"]`) and `outry vars` and the app will point out the
ones that are missing.

In the app, use **Add secret** at the bottom of the sidebar. In CI, set `OUTRY_TOKEN=...` instead.

## Running in CI

```yaml
# .github/workflows/api-tests.yml
- name: Install outry
  run: |
    curl -fsSL https://raw.githubusercontent.com/1rowvy/outry/master/install.sh | sh
    echo "$HOME/.local/bin" >> "$GITHUB_PATH"

- name: Lint request files
  run: outry check api --format github

- name: Run API tests
  run: outry run api --env ci --no-keyring --fail-fast
  env:
    OUTRY_TOKEN: ${{ secrets.API_TOKEN }}
```

Exit codes: `0` everything passed, `1` at least one request failed, `2` Outry could not start.
Use `--json` for JSON Lines output, one object per request. See the [CI guide](https://1rowvy.github.io/outry/guides/ci/).

## CLI reference

```sh
outry init [DIR]                       # create api/env.toml and an example request
outry run <PATHS|NAMES>...             # send requests; folders run alphabetically, names and file:line too
    -e, --env <ENV>                    #   environment from env.toml
    --var <NAME=VALUE>                 #   override a variable (repeatable)
    --fail-fast                        #   stop at the first failure
    -v, --verbose                      #   print response headers and body
    --json                             #   JSON Lines output for CI
    --fresh                            #   ignore values saved by previous runs
    --no-keyring                       #   secrets only from OUTRY_*
    --yes                              #   send `confirm: true` requests without asking
outry check [PATHS]...                 # nothing is sent. Without paths — the whole project: syntax,
                                       #   names, arguments, shapes, cycles, every environment (`only`,
                                       #   missing variables), formatting, Go routes if there is a go.mod
    -e, --env <ENV>                    #   one environment, with keychain and OUTRY_* values
    --no-fmt, --no-go, --go <DIR>      #   skip formatting / Go, or point to the Go service
    --format github                    #   GitHub Actions annotations
outry fmt [PATHS]... [--check]         # canonical style for *.outry, like gofmt
outry convert [PATHS]... [--rm]        # *.http → *.outry
outry vars [-e ENV] [--reveal]         # final variable values and their sources
outry envs                             # list environments (* = default)
outry import go [DIR] [--check|--fix]  # create requests for Go routes, compare and fix existing ones
outry lsp [--env ENV]                  # language server for editors (stdio)
outry secret set|rm <NAME> [-e ENV]    # manage keychain secrets
outry update [--check]                 # self-update from GitHub releases
```

All commands and flags: [CLI reference](https://1rowvy.github.io/outry/reference/cli/).

## Desktop app

- A file tree of every `*.outry` and `*.http` in the project with rename / move / delete, plus an environment switcher
- An editor with highlighting, live `outry check` and completion of requests, variables and functions.
  <kbd>Ctrl</kbd>+<kbd>Enter</kbd> (or ▶ next to a request) runs the request or flow at the cursor, <kbd>Ctrl</kbd>+<kbd>S</kbd> saves
- A trace of the requests a request or flow called, with what came from the run's cache
- Requests run in parallel and can be cancelled
- The response shows highlighted, searchable body, image and HTML previews, headers, test results and the
  exact request that was sent; the body can be saved to a file
- Response history (in memory, or on disk outside the repo) and a variables panel showing where every value comes from
- A built-in `env.toml` editor with TOML validation
- Routes tab: syncs request files with the routes of a Go service
- Changes made outside the app (VS Code, `git pull`) show up immediately

More in the [desktop app guide](https://1rowvy.github.io/outry/guides/desktop-app/).

## Editors

**VS Code** (and Cursor, Windsurf, VSCodium): the [Outry extension](editors/vscode) bundles `outry` and adds a
response view beside the file (the same as in the desktop app), “Copy as curl”, the environment in the status bar,
and the project's requests and variables in the sidebar.

`outry lsp` brings the same checks to Neovim, Helix and any editor with an LSP client: `outry check` errors
as you type, differences from the Go code with quick fixes, completion of requests, variables and
functions, hover with variable values (secrets masked), go to definition (a call → the request, a variable →
`env.toml`, a shape → the Go struct) and a “▶ Send” code lens that opens the response next to the request.
Highlighting comes from the tree-sitter grammar in [`editors/tree-sitter-outry`](editors/tree-sitter-outry).
Setup: [editors guide](https://1rowvy.github.io/outry/guides/editors/).

## Development

```
crates/outry-core   .outry parser, checker, formatter and executor; .http support, variables, keychain,
                    Go route import: all behavior lives here
crates/outry-cli    the `outry` binary, a thin wrapper over core
app/                Tauri 2 + React desktop app, also a thin wrapper over core
editors/vscode      VS Code extension: `outry lsp` client; the response webview reuses app/src components
editors/tree-sitter-outry
                    tree-sitter grammar for *.outry (highlighting in Neovim, Helix, Zed)
docs/               Astro Starlight documentation site (English + Russian)
examples/api        sample requests against httpbin.org
```

```sh
cargo test -p outry-core -p outry-cli
cargo run -p outry-cli -- run examples/api --fresh --no-keyring

cd app && npm install && npm run tauri dev
```

The desktop app on Linux needs `webkit2gtk-4.1` (see [`.github/workflows/ci.yml`](.github/workflows/ci.yml)).
The roadmap and the release process are in [PLAN.md](PLAN.md).

## License

[MIT](LICENSE)
