#!/bin/sh
# Установка Routy CLI:
#   curl -fsSL https://raw.githubusercontent.com/1rowvy/routy/master/install.sh | sh
#
# Переменные:
#   ROUTY_VERSION      версия, например v0.2.0 (по умолчанию — последний релиз)
#   ROUTY_INSTALL_DIR  куда положить бинарь (по умолчанию ~/.local/bin)
#
# Обновление потом: `routy update`. Удаление: rm ~/.local/bin/routy

set -eu

REPO="1rowvy/routy"

say() { printf '%s\n' "$*"; }
err() { printf 'routy install: %s\n' "$*" >&2; exit 1; }

# Всё в функции: если скачивание скрипта оборвётся на середине, sh не выполнит половину.
main() {
    [ "$(uname -s)" = "Linux" ] || err "пока поддерживается только Linux (у вас $(uname -s))"

    case "$(uname -m)" in
        x86_64 | amd64) arch="x86_64" ;;
        aarch64 | arm64) arch="aarch64" ;;
        *) err "архитектура $(uname -m) не поддерживается" ;;
    esac
    # Статическая сборка: работает на любом дистрибутиве, включая Alpine.
    target="${arch}-unknown-linux-musl"

    if command -v curl >/dev/null 2>&1; then
        fetch() { curl -fsSL "$1" -o "$2"; }
        fetch_stdout() { curl -fsSL "$1"; }
    elif command -v wget >/dev/null 2>&1; then
        fetch() { wget -qO "$2" "$1"; }
        fetch_stdout() { wget -qO- "$1"; }
    else
        err "нужен curl или wget"
    fi

    if command -v sha256sum >/dev/null 2>&1; then
        sha256() { sha256sum "$1" | cut -d' ' -f1; }
    elif command -v shasum >/dev/null 2>&1; then
        sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
    else
        err "нужен sha256sum или shasum для проверки архива"
    fi

    version="${ROUTY_VERSION:-}"
    if [ -z "$version" ]; then
        version="$(fetch_stdout "https://api.github.com/repos/$REPO/releases/latest" |
            sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)"
        [ -n "$version" ] || err "не удалось узнать последнюю версию (лимит GitHub API? задайте ROUTY_VERSION)"
    fi
    case "$version" in v*) ;; *) version="v$version" ;; esac

    name="routy-cli-${version}-${target}"
    url="https://github.com/$REPO/releases/download/${version}/${name}.tar.gz"
    dir="${ROUTY_INSTALL_DIR:-$HOME/.local/bin}"

    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT INT TERM

    say "routy ${version} (${target})"
    fetch "$url" "$tmp/routy.tar.gz" || err "не удалось скачать $url"
    fetch "$url.sha256" "$tmp/routy.tar.gz.sha256" || err "не удалось скачать контрольную сумму $url.sha256"

    expected="$(cut -d' ' -f1 <"$tmp/routy.tar.gz.sha256")"
    actual="$(sha256 "$tmp/routy.tar.gz")"
    [ "$expected" = "$actual" ] || err "sha256 не совпадает: ожидали $expected, получили $actual"

    tar -xzf "$tmp/routy.tar.gz" -C "$tmp"
    mkdir -p "$dir"
    # Через временный файл и mv: не ломаем запущенный routy и не оставляем половину файла.
    cp "$tmp/$name/routy" "$dir/.routy.new"
    chmod 755 "$dir/.routy.new"
    mv -f "$dir/.routy.new" "$dir/routy"

    say "установлен в $dir/routy"
    case ":$PATH:" in
        *":$dir:"*) "$dir/routy" --version ;;
        *)
            say ""
            say "$dir нет в PATH. Добавьте в профиль оболочки:"
            say "  bash/zsh: echo 'export PATH=\"$dir:\$PATH\"' >> ~/.bashrc"
            say "  fish:     fish_add_path $dir"
            ;;
    esac
}

main "$@"
