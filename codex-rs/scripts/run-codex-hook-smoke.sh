#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKTREE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$WORKTREE_ROOT"

export CODEXN_ROOT="${CODEXN_ROOT:-$WORKTREE_ROOT}"
export NERO_HOOK_SMOKE_MODE="${NERO_HOOK_SMOKE_MODE:-nero_status_block_probe}"

exec /workspace/purrnet/scripts/codexn --hook-debug --dev "$@"
