#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if [[ -n "${DEVBOX_TEST_POSTGRES_URL:-}" ]]; then
  cargo test -p devbox-metadata postgres_store_matches_sqlite_core_semantics_when_configured -- --nocapture
  exit 0
fi

if ! command -v docker >/dev/null 2>&1; then
  echo "DEVBOX_TEST_POSTGRES_URL is not set and docker was not found." >&2
  echo "Set DEVBOX_TEST_POSTGRES_URL=postgres://user:pass@host:5432/db or install Docker/OrbStack." >&2
  exit 1
fi

CONTAINER_NAME="devbox-postgres-test-$$"
CONTAINER_ID="$(
  docker run -d \
    --name "$CONTAINER_NAME" \
    -e POSTGRES_USER=devbox \
    -e POSTGRES_PASSWORD=devbox \
    -e POSTGRES_DB=devbox_metadata_test \
    -p 127.0.0.1::5432 \
    postgres:16
)"

cleanup() {
  docker rm -f "$CONTAINER_ID" >/dev/null 2>&1 || true
}
trap cleanup EXIT

for _ in $(seq 1 30); do
  if docker exec "$CONTAINER_ID" pg_isready -U devbox -d devbox_metadata_test >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

if ! docker exec "$CONTAINER_ID" pg_isready -U devbox -d devbox_metadata_test >/dev/null 2>&1; then
  echo "Postgres container did not become ready." >&2
  exit 1
fi

HOST_PORT="$(docker port "$CONTAINER_ID" 5432/tcp | awk -F: 'END { print $NF }')"
if [[ -z "$HOST_PORT" ]]; then
  echo "Could not determine published Postgres port." >&2
  exit 1
fi

export DEVBOX_TEST_POSTGRES_URL="postgres://devbox:devbox@127.0.0.1:${HOST_PORT}/devbox_metadata_test"
cargo test -p devbox-metadata postgres_store_matches_sqlite_core_semantics_when_configured -- --nocapture
