#!/usr/bin/env bash
# Serial resource-limited checks with cache housekeeping before and afterwards.
# check.sh cargo clippy --workspace --all-targets --locked -- -D warnings
# With no arguments, runs the workspace tests.
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ ${CODEX_CHECK_ACTIVE:-} != 1 && -x "$HOME/.local/bin/codex-check" ]]; then
  exec "$HOME/.local/bin/codex-check" bash "$0" "$@"
fi
# Node launched through a shell/npm still needs the wrapper's 1 GiB heap cap.
export NODE_OPTIONS="${NODE_OPTIONS:+$NODE_OPTIONS }--max-old-space-size=1024"
scripts/clean.sh --if-large
if (( $# == 0 )); then set -- cargo test --workspace --locked; fi
status=0
"$@" || status=$?
scripts/clean.sh --if-large
exit "$status"
