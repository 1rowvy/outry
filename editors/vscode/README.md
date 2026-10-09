# Outry for VS Code

[Outry](https://1rowvy.github.io/outry/) keeps API requests as plain `*.outry` files in your repository,
next to the code — reviewed in pull requests, run in CI with the `outry` CLI. This extension is the
editor for them: everything it shows comes from `outry lsp`, the same checks and names as
`outry check`, the CLI and the desktop app.

```outry
CreateOrder: POST /orders {
  headers { Authorization: "Bearer ${Login().body.token}" }
  body { sku: "A-1", qty: 2 }
  expect { status == 201 }
  save order_id = body.id
}
```

## Features

- **▶ Send** above every request (**▶ Run flow** above flows), or <kbd>Ctrl</kbd>+<kbd>Enter</kbd> on any of its lines.
  The response opens beside the file: body with highlighting and search, preview of images and HTML,
  headers, checks from `expect`, the trace of called requests (`Login()`), the request as it was sent.
- **in…** — send in another environment once, without switching. `confirm: true` asks first.
- **Copy as curl** — the request with variables, calls and cookies resolved.
- **Environment** in the status bar; the choice is remembered per workspace.
- **Sidebar** — every request and flow of the project by folder, and the variables of the current environment
  with where each comes from (`env.toml`, keychain, `OUTRY_*`, `save`); secrets are masked.
- **Errors as you type**, environment warnings, **differences from the Go code** with quick fixes,
  **completion**, **hover**, **go to definition**, **outline**, **format document**.

## Requirements

The marketplace packages for Linux, macOS and Windows include the `outry` binary. Elsewhere, install the CLI
([instructions](https://1rowvy.github.io/outry/install/)) — the extension finds it on `PATH`, in `~/.local/bin`,
or at `outry.path`.

## Settings

| Setting | Default | |
|---------|---------|---|
| `outry.path` | bundled, then `PATH` | Path to the `outry` binary |
| `outry.env` | `default` from `env.toml` | Environment to start with |
| `outry.keyring` | `true` | Read secrets from the system keychain |
| `outry.import` | `true` | Compare requests with the Go code |
| `outry.goDir` | the parent of `api/` | Folder with the Go code, relative to the workspace |

Documentation: [outry-format](https://1rowvy.github.io/outry/reference/outry-format/),
[editors](https://1rowvy.github.io/outry/guides/editors/).
