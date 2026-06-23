#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$repo_root"

with_desktop=false
if [[ "${1:-}" == "--with-desktop" ]]; then
  with_desktop=true
fi

pids=()

cleanup() {
  if ((${#pids[@]} > 0)); then
    kill "${pids[@]}" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

run_service() {
  local name="$1"
  shift
  echo "Starting $name..."
  "$@" &
  pids+=("$!")
}

run_service "Bindhub API" pnpm dev:api
run_service "Bindhub dashboard" pnpm dev:web
run_service "Bindhub public site" pnpm dev:site

if [[ "$with_desktop" == "true" ]]; then
  run_service "Bindhub desktop renderer" pnpm dev:desktop
fi

cat <<'INFO'

Bindhub local stack started.
API:       http://127.0.0.1:3001
Dashboard: http://localhost:3000
Site/docs: http://localhost:3002

Press Ctrl+C to stop the stack.
Use ./start.sh --with-desktop to also start the desktop renderer.

INFO

wait
