#!/bin/sh
set -eu

if ! command -v cargo >/dev/null 2>&1; then
    printf '%s\n' 'install.sh: Install Rust and Cargo and make sure cargo is on PATH.' >&2
    exit 1
fi

source_dir=$(CDPATH= cd -P "$(dirname "$0")" && pwd -P)
install_root=${HOME:?HOME must be set}/.local
binary=$install_root/bin/lintroller

cargo install --locked --path "$source_dir" --bin lintroller --root "$install_root"

printf 'Installed lintroller to %s\n' "$binary"
if [ "$(command -v lintroller || true)" != "$binary" ]; then
    printf '%s\n' 'To run lintroller by name, put the install directory first on PATH:' \
        '  export PATH="$HOME/.local/bin:$PATH"'
fi
