#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

export CODEXN_CONFIG_NERO_MSG_PATH="${CODEXN_CONFIG_NERO_MSG_PATH:-$HOME/.codex/config-nero-hook-msg.toml}"
export CODEXN_CONFIG_NERO_AUTO_PATH="${CODEXN_CONFIG_NERO_AUTO_PATH:-$HOME/.codex/config-nero-hook-auto.toml}"
export NERO_RUNTIME_STATE_CONTROL_MODULE="${NERO_RUNTIME_STATE_CONTROL_MODULE:-nero_hook_runtime.session_auto_bridge}"
export NEROBAR_NERO_RUNTIME_STATE_CONTROL_MODULE="${NEROBAR_NERO_RUNTIME_STATE_CONTROL_MODULE:-nero_hook_runtime.session_auto_bridge}"

cargo run -p codex-cli --bin codex --
