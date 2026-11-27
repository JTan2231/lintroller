#!/bin/sh
set -eu

if ! command -v cargo >/dev/null 2>&1; then
    printf '%s\n' 'install.sh: Install Rust and Cargo and make sure cargo is on PATH.' >&2
    exit 1
fi

source_dir=$(CDPATH= cd -P "$(dirname "$0")" && pwd -P)
install_root=${HOME:?HOME must be set}/.local
binary=$install_root/bin/lintroller
man_dir=$install_root/share/man/man1

cargo install --locked --path "$source_dir" --bin lintroller --root "$install_root"
install -d "$man_dir"
install -m 644 "$source_dir/man/lintroller.1" "$man_dir/lintroller.1"

printf 'Installed lintroller to %s\n' "$binary"
printf 'Installed lintroller(1) to %s\n' "$man_dir/lintroller.1"
if [ "$(command -v lintroller || true)" != "$binary" ]; then
    printf '%s\n' 'To run lintroller by name, put the install directory first on PATH:' \
        '  export PATH="$HOME/.local/bin:$PATH"'
fi
