#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
systemctl --user start misc-tools-comfyui.service || true
exec "$repo_root/.agents/tools/cargo.sh" run --locked --package image-studio -- "$@"
