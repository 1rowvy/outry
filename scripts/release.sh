#!/usr/bin/env bash
# Поднимает версию во всех местах. Коммит и тег — вручную (команды печатаются в конце).
#   scripts/release.sh 0.2.0
set -euo pipefail

version="${1:?usage: scripts/release.sh X.Y.Z}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || { echo "bad version: $version" >&2; exit 1; }
root="$(cd "$(dirname "$0")/.." && pwd)"

# [workspace.package] version — единственная версия Rust-крейтов и приложения.
sed -i.bak -E "0,/^version = \"[^\"]+\"/s//version = \"$version\"/" "$root/Cargo.toml" && rm "$root/Cargo.toml.bak"
(cd "$root/app" && npm version "$version" --no-git-tag-version --allow-same-version >/dev/null)
(cd "$root/editors/vscode" && npm version "$version" --no-git-tag-version --allow-same-version >/dev/null)
(cd "$root" && cargo update --workspace --quiet)

echo "version → $version"
echo
echo "  git commit -am \"release v$version\""
echo "  git tag v$version"
echo "  git push && git push origin v$version"
