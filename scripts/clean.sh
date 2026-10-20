#!/usr/bin/env bash
# Keep local Cargo caches within a budget; packaged apps in dist/ are retained.
# clean.sh              clear development/test builds, retain release caches
# clean.sh --all        clear all Cargo caches
# clean.sh --if-large   enforce budgets and retain the two newest APKs per variant
# clean.sh --status     report cache sizes without changing anything
set -euo pipefail
cd "$(dirname "$0")/.."
mode=${1:-dev}
case "$mode" in dev|--all|--if-large|--status) ;; *) echo 'usage: scripts/clean.sh [--all|--if-large|--status]' >&2; exit 2;; esac
if [[ "$mode" != --status && ${CODEX_CHECK_ACTIVE:-} != 1 && -x "$HOME/.local/bin/codex-check" ]]; then
  exec "$HOME/.local/bin/codex-check" bash "$0" "$@"
fi
# Never follow a target/ symlink into another project's cache. Explicit paths
# also prevent CARGO_TARGET_DIR from redirecting a clean outside this workspace.
[[ ! -L target ]] || { echo 'Refusing to clean a symlinked target directory' >&2; exit 1; }
target="$PWD/target"
dev_limit=${SLIPSTREAM_DEV_CACHE_GIB:-12}
release_limit=${SLIPSTREAM_RELEASE_CACHE_GIB:-32}
[[ "$dev_limit" =~ ^[1-9][0-9]*$ && "$release_limit" =~ ^[1-9][0-9]*$ ]] || { echo 'Cache budgets must be positive whole GiB' >&2; exit 2; }
size_kib() {
  { du -sk "$@" 2>/dev/null || true; } | awk '{n+=$1} END {printf "%.0f\n",n}'
}
profile_size() { size_kib "$target/$1" "$target"/*/"$1"; }
report() {
  awk -v dev="$(profile_size debug)" -v rel="$(profile_size release)" \
      -v dl="$dev_limit" -v rl="$release_limit" \
      'BEGIN {printf "Cargo cache: %.2f GiB development/test (budget %s), %.2f GiB release (budget %s)\n",dev/1048576,dl,rel/1048576,rl}'
}
ensure_cache_tag() {
if [[ -d "$target" && ! -e "$target/CACHEDIR.TAG" && ! -L "$target/CACHEDIR.TAG" ]]; then
  # Cargo now validates explicit target directories. Migrate an older cache
  # only when its compiler metadata and fingerprints identify it as Cargo data.
  python3 - <<'PY'
from pathlib import Path
import json
target = Path('target')
metadata = target / '.rustc_info.json'
if not metadata.is_file() or metadata.is_symlink():
    raise SystemExit('Refusing to mark an unrecognized target directory as a Cargo cache')
info = json.loads(metadata.read_text())
if 'rustc_fingerprint' not in info or not any(p.is_dir() for pattern in ['*/.fingerprint', '*/*/.fingerprint'] for p in target.glob(pattern)):
    raise SystemExit('Refusing to mark an unrecognized target directory as a Cargo cache')
with (target / 'CACHEDIR.TAG').open('x') as stream:
    stream.write('Signature: 8a477f597d28d172789f06886806bc55\n# Cargo build cache\n')
PY
fi
}
clean_profile() {
  local profile=$1 folder=$2 path
  [[ -d "$target" ]] || return 0
  ensure_cache_tag
  cargo clean --target-dir "$target" --profile "$profile"
  for path in "$target"/*/"$folder"; do
    [[ -d "$path" ]] || continue
    # Cargo's profile-wide clean ignores --target on this toolchain. Remove
    # only recognized cross-target profile directories under our check lock.
    [[ ! -L "$path" && -d "$path/.fingerprint" ]] || { echo "Unrecognized cache: $path" >&2; exit 1; }
    echo "Removing cross-platform cache: $path"
    rm -rf -- "$path"
  done
}
report
case "$mode" in
  --status) exit 0 ;;
  --all) ensure_cache_tag; cargo clean --target-dir "$target" ;;
  dev) clean_profile dev debug ;;
  --if-large)
    if (( $(profile_size debug) > dev_limit * 1048576 )); then clean_profile dev debug; fi
    if (( $(profile_size release) > release_limit * 1048576 )); then clean_profile release release; fi
    ;;
esac
# Only generated APK names are eligible. Keep the latest two semantic versions
# separately for release/debug; the assembled station and unknown files stay.
python3 - <<'PY'
from pathlib import Path
import re
variants = {}
for path in Path('dist/android').glob('slipstream-remote-*.apk'):
    match = re.fullmatch(r'slipstream-remote-(\d+)\.(\d+)\.(\d+)(-debug)?\.apk', path.name)
    if match and path.is_file() and not path.is_symlink():
        variants.setdefault(match[4] or 'release', []).append((tuple(map(int, match.group(1, 2, 3))), path))
for packages in variants.values():
    for _, path in sorted(packages, reverse=True)[2:]:
        print(f'Removing old generated package: {path}')
        path.unlink()
PY
report
