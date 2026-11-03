# Contributing

Thanks for your interest in Outry!

- **Bugs**: open an [issue](https://github.com/1rowvy/outry/issues/new/choose) with the version, OS and a minimal `.outry` file.
- **Questions and ideas**: use [Discussions](https://github.com/1rowvy/outry/discussions).
- **Security issues**: see [SECURITY.md](SECURITY.md).

## Pull requests

For anything bigger than a small fix, open an issue or a discussion first so we can agree on the
approach. All behavior lives in `crates/outry-core`; the CLI, the desktop app and the VS Code extension
are thin wrappers over it. See [Development](README.md#development) for the layout.

Before opening a PR, run what CI runs:

```sh
cargo fmt --all --check
cargo clippy -p outry-core -p outry-cli --all-targets -- -D warnings
cargo test -p outry-core -p outry-cli
```

If you change the desktop app: `cd app && npm run build`. If you change the docs, update both the
English pages and their Russian translations under `docs/src/content/docs/ru/`.

By contributing you agree that your contribution is licensed under the [MIT license](LICENSE).
