#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
package_root="$(cd "$script_dir/.." && pwd)"
env_file="${1:-.env.r2.local}"
if [[ -f "$env_file" ]]; then
  set -a
  # shellcheck disable=SC1090
  source "$env_file"
  set +a
fi

if [[ -n "${BINDHUB_DAEMON_BIN:-}" ]]; then
  daemon_bin="$BINDHUB_DAEMON_BIN"
elif [[ -x "$package_root/bindhub-daemon" ]]; then
  daemon_bin="$package_root/bindhub-daemon"
else
  daemon_bin="bindhub-daemon"
fi
mode="${BINDHUB_LIVE_MODE:-push}"
db="${BINDHUB_LIVE_DB:?set BINDHUB_LIVE_DB}"
cache="${BINDHUB_LIVE_CACHE:?set BINDHUB_LIVE_CACHE}"
project_root="${BINDHUB_LIVE_PROJECT_ROOT:?set BINDHUB_LIVE_PROJECT_ROOT}"
remote_kind="${BINDHUB_REMOTE_KIND:-local}"

args=(
  sync
  --db "$db"
  --cache "$cache"
)

if [[ "$remote_kind" == "hosted" ]]; then
  : "${BINDHUB_METADATA_API:?set BINDHUB_METADATA_API for hosted metadata/object-access API}"
  : "${BINDHUB_METADATA_PROJECT:?set BINDHUB_METADATA_PROJECT for hosted object-access scope}"
  : "${BINDHUB_SESSION_TOKEN:?set BINDHUB_SESSION_TOKEN with bindhub auth hosted-login}"
  object_access_lease="${BINDHUB_OBJECT_ACCESS_LEASE:-bindhub-managed}"
  args+=(
    --remote-kind hosted
    --object-access-api "$BINDHUB_METADATA_API"
    --object-access-session-token-env BINDHUB_SESSION_TOKEN
    --object-access-lease "$object_access_lease"
    --metadata-mode hosted-api
    --metadata-api "$BINDHUB_METADATA_API"
    --metadata-session-token-env BINDHUB_SESSION_TOKEN
    --metadata-project "$BINDHUB_METADATA_PROJECT"
  )
elif [[ "$remote_kind" == "s3" ]]; then
  : "${BINDHUB_METADATA_DB:?set BINDHUB_METADATA_DB for s3 live sync}"
  : "${BINDHUB_METADATA_PROJECT:?set BINDHUB_METADATA_PROJECT for s3 object-access scope}"
  args+=(
    --remote-kind s3
    --s3-endpoint "${BINDHUB_R2_ENDPOINT:?set BINDHUB_R2_ENDPOINT}"
    --s3-bucket "${BINDHUB_R2_BUCKET:?set BINDHUB_R2_BUCKET}"
    --s3-region "${BINDHUB_R2_REGION:-auto}"
    --s3-prefix "${BINDHUB_R2_PREFIX:?set BINDHUB_R2_PREFIX}"
    --s3-access-key-env BINDHUB_R2_ACCESS_KEY_ID
    --s3-secret-key-env BINDHUB_R2_SECRET_ACCESS_KEY
    --object-access-api "${BINDHUB_METADATA_API:?set BINDHUB_METADATA_API}"
    --object-access-session-token-env BINDHUB_SESSION_TOKEN
    --object-access-lease "${BINDHUB_OBJECT_ACCESS_LEASE:?set BINDHUB_OBJECT_ACCESS_LEASE}"
  )
  if [[ -n "${BINDHUB_R2_SESSION_TOKEN:-}" ]]; then
    args+=(--s3-session-token-env BINDHUB_R2_SESSION_TOKEN)
  fi
elif [[ "$remote_kind" == "local" ]]; then
  args+=(--remote "${BINDHUB_REMOTE_DIR:?set BINDHUB_REMOTE_DIR for local remote mode}")
else
  echo "BINDHUB_REMOTE_KIND must be local, hosted, or s3" >&2
  exit 2
fi

if [[ -n "${BINDHUB_METADATA_DB:-}" && "$remote_kind" != "hosted" ]]; then
  args+=(--metadata-mode mock-dev-sqlite --metadata-db "$BINDHUB_METADATA_DB")
  if [[ -n "${BINDHUB_METADATA_ACCOUNT:-}" ]]; then
    args+=(--metadata-account "$BINDHUB_METADATA_ACCOUNT")
  fi
  if [[ -n "${BINDHUB_METADATA_PROJECT:-}" ]]; then
    args+=(--metadata-project "$BINDHUB_METADATA_PROJECT")
  fi
fi

case "$mode" in
  push) args+=(--push) ;;
  pull) args+=(--pull) ;;
  two-way) args+=(--two-way) ;;
  *) echo "BINDHUB_LIVE_MODE must be push, pull, or two-way" >&2; exit 2 ;;
esac

if [[ -n "${BINDHUB_LIVE_TARGET:-}" ]]; then
  args+=(--to "$BINDHUB_LIVE_TARGET")
fi
if [[ "${BINDHUB_LIVE_APPLY:-false}" == "true" ]]; then
  args+=(--apply)
fi
if [[ "${BINDHUB_LIVE_ONCE:-false}" == "true" ]]; then
  args+=(--once)
fi

args+=("$project_root")
exec "$daemon_bin" "${args[@]}"
