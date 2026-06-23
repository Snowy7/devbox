#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if [[ -n "${BINDHUB_TEST_POSTGRES_URL:-}" ]]; then
  cargo test -p bindhub-metadata postgres_store_matches_sqlite_core_semantics_when_configured -- --nocapture
  exit 0
fi

if ! command -v docker >/dev/null 2>&1; then
  echo "BINDHUB_TEST_POSTGRES_URL is not set and docker was not found." >&2
  echo "Set BINDHUB_TEST_POSTGRES_URL=postgres://user:pass@host:5432/db or install Docker/OrbStack." >&2
  exit 1
fi

CONTAINER_NAME="Bindhub-postgres-test-$$"
CONTAINER_ID="$(
  docker run -d \
    --name "$CONTAINER_NAME" \
    -e POSTGRES_USER=Bindhub \
    -e POSTGRES_PASSWORD=Bindhub \
    -e POSTGRES_DB=bindhub_metadata_test \
    -p 127.0.0.1::5432 \
    postgres:16
)"

cleanup() {
  docker rm -f "$CONTAINER_ID" >/dev/null 2>&1 || true
}
trap cleanup EXIT

for _ in $(seq 1 30); do
  if docker exec "$CONTAINER_ID" pg_isready -U Bindhub -d bindhub_metadata_test >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

if ! docker exec "$CONTAINER_ID" pg_isready -U Bindhub -d bindhub_metadata_test >/dev/null 2>&1; then
  echo "Postgres container did not become ready." >&2
  exit 1
fi

HOST_PORT="$(docker port "$CONTAINER_ID" 5432/tcp | awk -F: 'END { print $NF }')"
if [[ -z "$HOST_PORT" ]]; then
  echo "Could not determine published Postgres port." >&2
  exit 1
fi

export BINDHUB_TEST_POSTGRES_URL="postgres://Bindhub:Bindhub@127.0.0.1:${HOST_PORT}/bindhub_metadata_test"
cargo test -p bindhub-metadata postgres_store_matches_sqlite_core_semantics_when_configured -- --nocapture
