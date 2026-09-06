#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"
# Optional, ignored environment for machines using project-local build tools.
if [[ -f .tools/env.sh ]]; then source .tools/env.sh; fi
exec cargo "$@"
