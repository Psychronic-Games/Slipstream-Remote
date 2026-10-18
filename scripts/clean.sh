#!/usr/bin/env bash
# Frees the disk space Cargo's build cache (target/) keeps growing into:
# every build leaves old copies of compiled crates behind.
#
#   scripts/clean.sh        # development and test builds (rebuilt in a few minutes)
#   scripts/clean.sh --all  # everything, release builds too (the next station build starts over)
set -euo pipefail
cd "$(dirname "$0")/.."

du -sh target 2>/dev/null || { echo "target/ is already empty"; exit 0; }
if [ "${1:-}" = "--all" ]; then
  cargo clean
else
  cargo clean --profile dev
  # That leaves cross-compiled dev builds (target/<triple>/debug: Windows
  # tests under Wine, macOS type-checks).
  find target -mindepth 2 -maxdepth 2 -type d -name debug -prune -exec rm -rf {} +
  # Incremental caches of the release builds are never reused.
  find target -maxdepth 3 -type d -name incremental -prune -exec rm -rf {} +
fi
du -sh target 2>/dev/null || echo "target/ removed"
