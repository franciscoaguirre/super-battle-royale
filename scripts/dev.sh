#!/usr/bin/env bash
#
# Quick local multiplayer test harness.
#
# Starts the headless server on 127.0.0.1:<port> and launches two clients
# connected to it. Press Ctrl-C to tear everything down. Closing a single client
# window leaves the rest running.
#
# Usage:
#   scripts/dev.sh [port]      # port defaults to 5000
#
set -euo pipefail

PORT="${1:-5000}"
ADDR="127.0.0.1:${PORT}"

# Run from the repo root regardless of where this is invoked from.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# Build both feature sets up front so the backgrounded `cargo run`s below don't
# race on the build lock or compile concurrently. Cargo caches each feature set
# separately, so alternating between them afterwards doesn't recompile. We use
# `cargo run` (not the raw binaries) so Bevy resolves assets from the project
# root and the dynamic-linking dylib paths are set up automatically.
echo "==> Building headless server (no rendering) + client..."
cargo build --no-default-features --features server --bin server
cargo build --bin super-battle-royale

PIDS=()
cleanup() {
  echo
  echo "==> Shutting down..."
  if [ "${#PIDS[@]}" -gt 0 ]; then
    kill "${PIDS[@]}" 2>/dev/null || true
  fi
  wait 2>/dev/null || true
}
trap cleanup EXIT INT TERM

echo "==> Starting server on 0.0.0.0:${PORT}"
cargo run --no-default-features --features server --bin server -- "0.0.0.0:${PORT}" &
PIDS+=("$!")

# Give the server a moment to bind the UDP port before clients dial in.
sleep 1

for n in 1 2; do
  echo "==> Starting client ${n} -> ${ADDR}"
  cargo run --bin super-battle-royale -- "${ADDR}" &
  PIDS+=("$!")
done

echo "==> Running (server + 2 clients). Press Ctrl-C to stop."
wait
