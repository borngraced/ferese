#!/usr/bin/env bash
# Install already-built binaries. Run from any directory with sudo.
set -euo pipefail

repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
release_id=${1:?usage: install-session.sh RELEASE_ID}
[[ $release_id =~ ^[a-zA-Z0-9][a-zA-Z0-9._-]*$ ]] || exit 2
[[ $EUID == 0 ]] || { echo 'Run this installer with sudo.' >&2; exit 1; }

install_root=/usr/local/lib/ferese
release_dir=$install_root/releases/$release_id
session_entry=/usr/share/wayland-sessions/ferese.desktop
[[ ! -e $release_dir ]] || { echo "Release already exists: $release_dir" >&2; exit 1; }

# Refuse to overwrite unrelated installations.
for name in ferese ferese-shell feresectl ferese-session; do
    link=/usr/local/bin/$name
    if [[ -e $link || -L $link ]]; then
        [[ -L $link && $(readlink -- "$link") == "$install_root/current/$name" ]] || {
            echo "Unmanaged path exists: $link" >&2; exit 1;
        }
    fi
done
for link in current previous; do
    if [[ -e $install_root/$link || -L $install_root/$link ]]; then
        [[ -L $install_root/$link && $(readlink -- "$install_root/$link") == releases/* ]] || {
            echo "Unmanaged path exists: $install_root/$link" >&2; exit 1;
        }
    fi
done
if [[ -e $session_entry ]]; then
    cmp -s -- "$repo_dir/packaging/ferese.desktop" "$session_entry" || {
        echo "Different session entry exists: $session_entry" >&2; exit 1;
    }
fi
for name in ferese ferese-shell feresectl; do
    [[ -x $repo_dir/target/release/$name ]] || { echo "Missing release binary: $name" >&2; exit 1; }
done
desktop-file-validate "$repo_dir/packaging/ferese.desktop"

install -d -m 0755 -- "$release_dir" /usr/local/bin /usr/share/wayland-sessions
for name in ferese ferese-shell feresectl; do
    install -m 0755 -- "$repo_dir/target/release/$name" "$release_dir/$name"
done
install -m 0755 -- "$repo_dir/packaging/ferese-session" "$release_dir/ferese-session"
install -m 0644 -- "$repo_dir/packaging/config.toml" "$release_dir/config.example.toml"

if [[ -L $install_root/current ]]; then
    ln -sfn -- "$(readlink -- "$install_root/current")" "$install_root/previous"
fi
ln -s -- "releases/$release_id" "$install_root/current-$release_id"
mv -Tf -- "$install_root/current-$release_id" "$install_root/current"
for name in ferese ferese-shell feresectl ferese-session; do
    ln -sfn -- "$install_root/current/$name" "/usr/local/bin/$name"
done
install -m 0644 -- "$repo_dir/packaging/ferese.desktop" "$session_entry"
echo "Installed Ferese $release_id. Select Ferese in SDDM."
