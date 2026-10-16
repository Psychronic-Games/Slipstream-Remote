#!/usr/bin/env bash
# One-machine trial: starts a private server, runs this desktop as a host,
# and opens a second Slipstream window that connects to it.
#
# On Wayland your desktop will ask permission to share the screen and allow
# remote input — choose a screen and click "Share"/"Allow". The viewer window
# then shows your own desktop (expect a hall-of-mirrors effect).
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release -p slipstream -p slipstream-server
BIN=./target/release
T="${TMPDIR:-/tmp}/slipstream-trial"
mkdir -p "$T"
PASS="trial-password-123"

"$BIN/slipstream-server" --data-dir "$T/server" --public-host 127.0.0.1 \
  --port 21216 --relay-http '[::]:21217' >"$T/server.log" 2>&1 &
SERVER=$!
HOST=""
trap 'kill $SERVER $HOST 2>/dev/null || true' EXIT
until [ -s "$T/server/server-address.txt" ]; do sleep 0.2; done
ADDR=$(cat "$T/server/server-address.txt")

for who in host viewer; do SLIPSTREAM_HOME="$T/$who" "$BIN/slipstream" set-server "$ADDR" >/dev/null; done
SLIPSTREAM_HOME="$T/host" "$BIN/slipstream" set-password "$PASS" >/dev/null

echo "==> Starting host window…"
SLIPSTREAM_HOME="$T/host" "$BIN/slipstream" ${HOST_ARGS:-} >"$T/host.log" 2>&1 &
HOST=$!
until SLIPSTREAM_HOME="$T/host" "$BIN/slipstream" info | grep -q "device id    : [0-9]"; do sleep 0.3; done
ID=$(SLIPSTREAM_HOME="$T/host" "$BIN/slipstream" info | sed -n 's/device id    : //p' | tr -d ' ')
echo "==> Host is online as $ID — opening the viewer (approve screen sharing when asked)"
SLIPSTREAM_HOME="$T/viewer" "$BIN/slipstream" --connect "$ID" --password "$PASS" ${VIEWER_ARGS:-} 2>"$T/viewer.log"
echo "==> Done. Logs are in $T"
