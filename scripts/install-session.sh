#!/usr/bin/env bash
# Apply a prepared bundle, roll back, or recover an interrupted installation.
set -euo pipefail
repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
exec python3 "$repo_dir/scripts/installer/install.py" "$@"
