#!/usr/bin/env bash
set -euo pipefail

env_file="${1:-.env.r2.local}"

if [[ ! -f "$env_file" ]]; then
  echo "missing env file: $env_file" >&2
  echo "copy .env.example to .env.r2.local, fill it in, then source this script" >&2
  return 1 2>/dev/null || exit 1
fi

set -a
# shellcheck disable=SC1090
source "$env_file"
set +a

echo "loaded R2 env from $env_file"
echo "bucket: ${DEVBOX_R2_BUCKET:-}"
echo "prefix: ${DEVBOX_R2_PREFIX:-}"
