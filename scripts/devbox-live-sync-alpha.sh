#!/usr/bin/env bash
set -euo pipefail

env_file="${1:-.env.r2.local}"
if [[ -f "$env_file" ]]; then
  set -a
  # shellcheck disable=SC1090
  source "$env_file"
  set +a
fi

daemon_bin="${DEVBOX_DAEMON_BIN:-devbox-daemon}"
mode="${DEVBOX_LIVE_MODE:-push}"
db="${DEVBOX_LIVE_DB:?set DEVBOX_LIVE_DB}"
cache="${DEVBOX_LIVE_CACHE:?set DEVBOX_LIVE_CACHE}"
project_root="${DEVBOX_LIVE_PROJECT_ROOT:?set DEVBOX_LIVE_PROJECT_ROOT}"
remote_kind="${DEVBOX_REMOTE_KIND:-local}"

args=(
  sync
  --db "$db"
  --cache "$cache"
)

if [[ "$remote_kind" == "s3" ]]; then
  : "${DEVBOX_METADATA_DB:?set DEVBOX_METADATA_DB for s3 live sync}"
  : "${DEVBOX_METADATA_PROJECT:?set DEVBOX_METADATA_PROJECT for s3 object-access scope}"
  args+=(
    --remote-kind s3
    --s3-endpoint "${DEVBOX_R2_ENDPOINT:?set DEVBOX_R2_ENDPOINT}"
    --s3-bucket "${DEVBOX_R2_BUCKET:?set DEVBOX_R2_BUCKET}"
    --s3-region "${DEVBOX_R2_REGION:-auto}"
    --s3-prefix "${DEVBOX_R2_PREFIX:?set DEVBOX_R2_PREFIX}"
    --s3-access-key-env DEVBOX_R2_ACCESS_KEY_ID
    --s3-secret-key-env DEVBOX_R2_SECRET_ACCESS_KEY
    --object-access-api "${DEVBOX_METADATA_API:?set DEVBOX_METADATA_API}"
    --object-access-session-token-env DEVBOX_SESSION_TOKEN
    --object-access-lease "${DEVBOX_OBJECT_ACCESS_LEASE:?set DEVBOX_OBJECT_ACCESS_LEASE}"
  )
  if [[ -n "${DEVBOX_R2_SESSION_TOKEN:-}" ]]; then
    args+=(--s3-session-token-env DEVBOX_R2_SESSION_TOKEN)
  fi
else
  args+=(--remote "${DEVBOX_REMOTE_DIR:?set DEVBOX_REMOTE_DIR for local remote mode}")
fi

if [[ -n "${DEVBOX_METADATA_DB:-}" ]]; then
  args+=(--metadata-mode mock-dev-sqlite --metadata-db "$DEVBOX_METADATA_DB")
  if [[ -n "${DEVBOX_METADATA_ACCOUNT:-}" ]]; then
    args+=(--metadata-account "$DEVBOX_METADATA_ACCOUNT")
  fi
  if [[ -n "${DEVBOX_METADATA_PROJECT:-}" ]]; then
    args+=(--metadata-project "$DEVBOX_METADATA_PROJECT")
  fi
fi

case "$mode" in
  push) args+=(--push) ;;
  pull) args+=(--pull) ;;
  two-way) args+=(--two-way) ;;
  *) echo "DEVBOX_LIVE_MODE must be push, pull, or two-way" >&2; exit 2 ;;
esac

if [[ -n "${DEVBOX_LIVE_TARGET:-}" ]]; then
  args+=(--to "$DEVBOX_LIVE_TARGET")
fi
if [[ "${DEVBOX_LIVE_APPLY:-false}" == "true" ]]; then
  args+=(--apply)
fi
if [[ "${DEVBOX_LIVE_ONCE:-false}" == "true" ]]; then
  args+=(--once)
fi

args+=("$project_root")
exec "$daemon_bin" "${args[@]}"
