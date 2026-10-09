# Changelog

The extension is released together with `outry`: versions match, notes are in the
[GitHub releases](https://github.com/1rowvy/outry/releases).

## 0.10.0

- Renamed to Outry: the extension id is now `outry.outry-vscode` (install it again; `routy.routy-vscode` gets no more updates),
  the binary is `outry`, files are `*.outry`, settings are `outry.*`.

## 0.7.0

- Go import knows route middleware; `[import.middleware]` in `env.toml` adds their headers (quick fix for existing requests).

## 0.6.2

- Marketplace name: Routy API Client.

## 0.6.1

- The extension id is now `routy.routy-vscode`.
- A `routy` older than 0.6.0 is reported with an Update button instead of failing with `write EPIPE`.
- Requests and variables appear when `env.toml` is created while the editor is open (`routy import go`).

## 0.6.0

- First release: `outry lsp` client, response view, environments, requests and variables in the sidebar,
  Copy as curl, syntax highlighting.
