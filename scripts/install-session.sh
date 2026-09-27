#!/usr/bin/env bash
# Install already-built binaries. Run from any directory with sudo.
set -euo pipefail

session_entry_matches() {
    # App-icon metadata can change between managed releases. All other fields,
    # especially Exec/TryExec, must still match before replacing an entry.
    cmp -s -- <(sed '/^Icon=/d' "$1") <(sed '/^Icon=/d' "$2")
}

repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
release_id=${1:?usage: install-session.sh RELEASE_ID}
[[ $release_id =~ ^[a-zA-Z0-9][a-zA-Z0-9._-]*$ ]] || exit 2
[[ $EUID == 0 ]] || { echo 'Run this installer with sudo.' >&2; exit 1; }

install_root=/usr/local/lib/ferese
release_dir=$install_root/releases/$release_id
session_entry=/usr/share/wayland-sessions/ferese.desktop
[[ ! -e $release_dir ]] || { echo "Release already exists: $release_dir" >&2; exit 1; }

# Refuse to overwrite unrelated installations.
for name in ferese ferese-shell ferese-settings feresectl ferese-session ferese-lock ferese-screenshot; do
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
    session_entry_matches "$repo_dir/packaging/ferese.desktop" "$session_entry" || {
        echo "Different session entry exists: $session_entry" >&2; exit 1;
    }
fi
for name in ferese ferese-shell ferese-settings feresectl; do
    [[ -x $repo_dir/target/release/$name ]] || { echo "Missing release binary: $name" >&2; exit 1; }
done
desktop-file-validate "$repo_dir/packaging/ferese.desktop"
desktop-file-validate "$repo_dir/packaging/dev.ferese.Settings.desktop"

install -d -m 0755 -- "$release_dir" /usr/local/bin /usr/share/wayland-sessions
if [[ -e $session_entry ]]; then
    install -m 0644 -- "$session_entry" "$release_dir/session.previous.desktop"
fi
for name in ferese ferese-shell ferese-settings feresectl; do
    install -m 0755 -- "$repo_dir/target/release/$name" "$release_dir/$name"
done
install -m 0755 -- "$repo_dir/packaging/ferese-session" "$release_dir/ferese-session"
install -m 0755 -- "$repo_dir/packaging/ferese-lock" "$release_dir/ferese-lock"
install -m 0755 -- "$repo_dir/packaging/ferese-screenshot" "$release_dir/ferese-screenshot"
install -m 0644 -- "$repo_dir/packaging/config.kdl" "$release_dir/config.example.kdl"

if [[ -L $install_root/current ]]; then
    ln -sfn -- "$(readlink -- "$install_root/current")" "$install_root/previous"
fi
ln -s -- "releases/$release_id" "$install_root/current-$release_id"
mv -Tf -- "$install_root/current-$release_id" "$install_root/current"
for name in ferese ferese-shell ferese-settings feresectl ferese-session ferese-lock ferese-screenshot; do
    ln -sfn -- "$install_root/current/$name" "/usr/local/bin/$name"
done
install -m 0644 -- "$repo_dir/packaging/ferese.desktop" "$session_entry"
install -D -m 0644 -- "$repo_dir/packaging/dev.ferese.Settings.desktop" /usr/local/share/applications/dev.ferese.Settings.desktop
install -D -m 0644 -- "$repo_dir/packaging/icons/ferese.svg" /usr/local/share/icons/hicolor/scalable/apps/dev.ferese.Settings.svg
install -D -m 0644 -- "$repo_dir/packaging/icons/ferese.svg" /usr/local/share/icons/hicolor/scalable/apps/ferese.svg
if command -v update-desktop-database >/dev/null; then update-desktop-database /usr/local/share/applications || true; fi
echo "Installed Ferese $release_id. Select Ferese in SDDM."
