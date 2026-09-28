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
for name in ferese ferese-shell ferese-settings feresectl ferese-session ferese-lock ferese-polkit-agent ferese-screenshot xdg-desktop-portal-ferese ferese-record; do
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
for name in ferese ferese-shell ferese-settings feresectl ferese-lock ferese-polkit-agent xdg-desktop-portal-ferese ferese-record; do
    [[ -x $repo_dir/target/release/$name ]] || { echo "Missing release binary: $name" >&2; exit 1; }
done
"$repo_dir/target/release/ferese-polkit-agent" --check
for name in ferese.png ferese.svg; do
    [[ -f $repo_dir/assets/wallpapers/$name ]] || { echo "Missing wallpaper asset: $name" >&2; exit 1; }
done
# Preserve administrator-supplied policies. New installs use the distro's
# authentication and account stacks, never a permissive fallback.
pam_policy=generic
if [[ -f /etc/pam.d/system-auth ]]; then
    pam_policy=fedora
elif [[ -f /etc/pam.d/common-auth && -f /etc/pam.d/common-account ]]; then
    pam_policy=debian
elif [[ ! -f /etc/pam.d/login && ! -f /etc/pam.d/ferese-lock ]]; then
    echo 'No supported PAM stack found; install /etc/pam.d/ferese-lock first.' >&2
    exit 1
fi

# These files are owned by Ferese; preserve unrelated administrator configuration.
for entry in \
    'ferese.portal:portals/ferese.portal' \
    'ferese-portals.conf:ferese-portals.conf'; do
    source=${entry%%:*}
    destination=/usr/share/xdg-desktop-portal/${entry#*:}
    if [[ -e $destination ]] && ! cmp -s "$repo_dir/packaging/portal/$source" "$destination"; then
        echo "Unmanaged portal configuration exists: $destination" >&2; exit 1
    fi
done
service=/usr/local/share/dbus-1/services/org.freedesktop.impl.portal.desktop.ferese.service
if [[ -e $service ]] && ! cmp -s "$repo_dir/packaging/portal/$(basename "$service")" "$service"; then
    # Upgrade the exact activation file shipped before systemd supervision.
    if ! cmp -s <(printf '%s\n' '[D-BUS Service]' \
        'Name=org.freedesktop.impl.portal.desktop.ferese' \
        'Exec=/usr/local/lib/ferese/current/xdg-desktop-portal-ferese') "$service"; then
        echo "Unmanaged portal service exists: $service" >&2; exit 1
    fi
fi

portal_unit=/usr/local/lib/systemd/user/xdg-desktop-portal-ferese.service
if [[ -e $portal_unit ]] && ! cmp -s "$repo_dir/packaging/systemd/xdg-desktop-portal-ferese.service" "$portal_unit"; then
    echo "Unmanaged portal unit exists: $portal_unit" >&2; exit 1
fi

session_target=/usr/local/lib/systemd/user/ferese-session.target
if [[ -e $session_target ]] && ! cmp -s "$repo_dir/packaging/systemd/ferese-session.target" "$session_target"; then
    echo "Unmanaged session target exists: $session_target" >&2; exit 1
fi

desktop-file-validate "$repo_dir/packaging/ferese.desktop"
desktop-file-validate "$repo_dir/packaging/dev.ferese.Settings.desktop"

install -d -m 0755 -- "$release_dir" /usr/local/bin /usr/share/wayland-sessions
if [[ -e $session_entry ]]; then
    install -m 0644 -- "$session_entry" "$release_dir/session.previous.desktop"
fi
for name in ferese ferese-shell ferese-settings feresectl ferese-lock ferese-polkit-agent xdg-desktop-portal-ferese ferese-record; do
    install -m 0755 -- "$repo_dir/target/release/$name" "$release_dir/$name"
done
install -m 0755 -- "$repo_dir/packaging/ferese-session" "$release_dir/ferese-session"
install -m 0755 -- "$repo_dir/packaging/ferese-session-shell" "$release_dir/ferese-session-shell"
install -D -m 0644 -- "$repo_dir/packaging/systemd/ferese-session.target" "$session_target"
install -D -m 0644 -- "$repo_dir/packaging/systemd/xdg-desktop-portal-ferese.service" "$portal_unit"
install -m 0755 -- "$repo_dir/packaging/ferese-screenshot" "$release_dir/ferese-screenshot"
install -m 0644 -- "$repo_dir/packaging/config.kdl" "$release_dir/config.example.kdl"
install -D -m 0644 -- "$repo_dir/assets/fonts/Comfortaa-LICENSE.txt" "$release_dir/licenses/Comfortaa-LICENSE.txt"
install -D -m 0644 -- "$repo_dir/assets/fonts/Cantarell-LICENSE.txt" "$release_dir/licenses/Cantarell-LICENSE.txt"
install -d -m 0755 -- "$release_dir/wallpapers"
install -m 0644 -- "$repo_dir/assets/wallpapers/ferese.png" "$release_dir/wallpapers/ferese.png"
install -m 0644 -- "$repo_dir/assets/wallpapers/ferese.svg" "$release_dir/wallpapers/ferese.svg"

if [[ ! -e /etc/pam.d/ferese-lock ]]; then
    install -D -m 0644 -- "$repo_dir/packaging/pam.d/ferese-lock.$pam_policy" /etc/pam.d/ferese-lock
fi

if [[ -L $install_root/current ]]; then
    ln -sfn -- "$(readlink -- "$install_root/current")" "$install_root/previous"
fi
ln -s -- "releases/$release_id" "$install_root/current-$release_id"
mv -Tf -- "$install_root/current-$release_id" "$install_root/current"
for name in ferese ferese-shell ferese-settings feresectl ferese-session ferese-lock ferese-polkit-agent ferese-screenshot xdg-desktop-portal-ferese ferese-record; do
    ln -sfn -- "$install_root/current/$name" "/usr/local/bin/$name"
done
install -m 0644 -- "$repo_dir/packaging/ferese.desktop" "$session_entry"
install -D -m 0644 -- "$repo_dir/packaging/dev.ferese.Settings.desktop" /usr/local/share/applications/dev.ferese.Settings.desktop
install -D -m 0644 -- "$repo_dir/packaging/icons/ferese.svg" /usr/local/share/icons/hicolor/scalable/apps/dev.ferese.Settings.svg
install -D -m 0644 -- "$repo_dir/packaging/icons/ferese.svg" /usr/local/share/icons/hicolor/scalable/apps/ferese.svg
install -D -m 0644 -- "$repo_dir/packaging/portal/ferese.portal" /usr/share/xdg-desktop-portal/portals/ferese.portal
install -D -m 0644 -- "$repo_dir/packaging/portal/ferese-portals.conf" /usr/share/xdg-desktop-portal/ferese-portals.conf
install -D -m 0644 -- "$repo_dir/packaging/portal/org.freedesktop.impl.portal.desktop.ferese.service" "$service"
if command -v update-desktop-database >/dev/null; then update-desktop-database /usr/local/share/applications || true; fi
echo "Installed Ferese $release_id. Select Ferese at your login screen, or run /usr/local/bin/ferese-session from a local TTY."
