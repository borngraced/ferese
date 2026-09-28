#!/usr/bin/env bash
# Build as the invoking user; elevate only the versioned system installation.
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: scripts/install.sh [options]

Build and install Ferese's compositor, shell, CLI, and login-screen session.
Run from any directory as your normal user. Existing user config is preserved.

  --release-id ID  Name the installed release (default: UTC timestamp + PID)
  --skip-build     Install existing target/release binaries
  --offline        Build using only cached Cargo dependencies
  --dry-run        Print commands without building or installing
  -h, --help       Show this help

The previous release remains available for rollback. Administrator
authentication is requested only after a successful build.
EOF
}

fail() { echo "ferese installer: $*" >&2; exit 1; }
repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
release_id="$(date -u +%Y%m%d-%H%M%S)-$$"
skip_build=false
offline=false
dry_run=false
while (($#)); do
    case $1 in
        --release-id)
            (($# >= 2)) || fail '--release-id requires a value'
            release_id=$2
            shift 2
            ;;
        --skip-build) skip_build=true; shift ;;
        --offline) offline=true; shift ;;
        --dry-run) dry_run=true; shift ;;
        -h|--help) usage; exit 0 ;;
        *) fail "unknown option: $1 (see --help)" ;;
    esac
done
[[ $release_id =~ ^[a-zA-Z0-9][a-zA-Z0-9._-]*$ ]] || fail 'invalid release ID'
[[ $(uname -s) == Linux ]] || fail 'Ferese requires Linux'
for tool in desktop-file-validate dbus-run-session; do
    command -v "$tool" >/dev/null || fail "required command not found: $tool"
done
desktop-file-validate "$repo_dir/packaging/ferese.desktop"

build=(cargo build --manifest-path "$repo_dir/Cargo.toml"
    --target-dir "$repo_dir/target" --release --locked
    -p ferese -p ferese-shell -p ferese-settings -p feresectl -p ferese-lock -p ferese-polkit-agent -p xdg-desktop-portal-ferese)
if $offline; then build+=(--offline); fi
if ! $skip_build; then
    ((EUID != 0)) || fail 'build as your normal user, without sudo (or use --skip-build)'
    command -v cargo >/dev/null || fail 'install the Rust toolchain and Cargo first'
fi

installer=(bash "$repo_dir/scripts/install-session.sh" "$release_id")
if ((EUID != 0)); then
    if command -v sudo >/dev/null; then
        installer=(sudo "${installer[@]}")
    elif command -v pkexec >/dev/null; then
        installer=(pkexec /usr/bin/bash "$repo_dir/scripts/install-session.sh" "$release_id")
    else
        fail 'system installation requires sudo or pkexec'
    fi
fi

if $dry_run; then
    if ! $skip_build; then printf 'Build: '; printf '%q ' "${build[@]}"; printf '\n'; fi
    printf 'Install: '; printf '%q ' "${installer[@]}"; printf '\n'
    exit 0
fi

cd -- "$repo_dir"
if ! $skip_build; then "${build[@]}"; fi
for name in ferese ferese-shell ferese-settings feresectl ferese-lock ferese-polkit-agent; do
    [[ -x $repo_dir/target/release/$name ]] || fail "missing release binary: $name"
done
"${installer[@]}"
echo 'Installation complete. Log out and select Ferese at your login screen, or run ferese-session from a local TTY.'
