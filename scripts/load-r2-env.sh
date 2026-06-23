#!/usr/bin/env bash
set -euo pipefail

env_file="${1:-.env.r2.local}"

if [[ ! -f "$env_file" ]]; then
  example_file=".env.example"
  if [[ -f ".env.operator.example" ]]; then
    example_file=".env.operator.example"
  fi
  echo "missing env file: $env_file" >&2
  echo "copy $example_file to .env.r2.local, fill in the server/operator R2 values, then source this script" >&2
  return 1 2>/dev/null || exit 1
fi

set -a
# shellcheck disable=SC1090
source "$env_file"
set +a

echo "loaded R2 env from $env_file"
echo "endpoint: ${BINDHUB_R2_ENDPOINT:-}"
echo "bucket: ${BINDHUB_R2_BUCKET:-}"
