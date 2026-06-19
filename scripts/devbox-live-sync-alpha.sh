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

if [[ -n "${DEVBOX_DAEMON_BIN:-}" ]]; then
  daemon_bin="$DEVBOX_DAEMON_BIN"
elif [[ -x "$package_root/devbox-daemon" ]]; then
  daemon_bin="$package_root/devbox-daemon"
else
  daemon_bin="devbox-daemon"
fi
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

if [[ "$remote_kind" == "hosted" ]]; then
  : "${DEVBOX_METADATA_API:?set DEVBOX_METADATA_API for hosted metadata/object-access API}"
  : "${DEVBOX_METADATA_PROJECT:?set DEVBOX_METADATA_PROJECT for hosted object-access scope}"
  : "${DEVBOX_OBJECT_ACCESS_LEASE:?set DEVBOX_OBJECT_ACCESS_LEASE for hosted object access}"
  : "${DEVBOX_SESSION_TOKEN:?set DEVBOX_SESSION_TOKEN with devbox auth hosted-login}"
  args+=(
    --remote-kind hosted
    --object-access-api "$DEVBOX_METADATA_API"
    --object-access-session-token-env DEVBOX_SESSION_TOKEN
    --object-access-lease "$DEVBOX_OBJECT_ACCESS_LEASE"
    --metadata-mode hosted-api
    --metadata-api "$DEVBOX_METADATA_API"
    --metadata-session-token-env DEVBOX_SESSION_TOKEN
    --metadata-project "$DEVBOX_METADATA_PROJECT"
  )
elif [[ "$remote_kind" == "s3" ]]; then
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
elif [[ "$remote_kind" == "local" ]]; then
  args+=(--remote "${DEVBOX_REMOTE_DIR:?set DEVBOX_REMOTE_DIR for local remote mode}")
else
  echo "DEVBOX_REMOTE_KIND must be local, hosted, or s3" >&2
  exit 2
fi

if [[ -n "${DEVBOX_METADATA_DB:-}" && "$remote_kind" != "hosted" ]]; then
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
