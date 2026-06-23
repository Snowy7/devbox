#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/alpha-two-device-smoke.sh

Runs a deterministic local two-device alpha smoke test:
  1. initialize a source device
  2. create a receiver DB through receiver-generated pairing
  3. verify the pending receiver fails closed before sync
  4. complete pairing
  5. publish with bindhub-daemon sync --once
  6. pull/materialize the latest hosted mock-dev snapshot into the receiver target

Environment:
  BINDHUB_BIN              Optional path to a built Bindhub binary.
  BINDHUB_DAEMON_BIN       Optional path to a built bindhub-daemon binary.
  BINDHUB_ALPHA_SMOKE_DIR  Optional working directory to reuse.
  BINDHUB_KEEP_SMOKE_DIR   Set true to keep a generated temp directory.
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ -n "${BINDHUB_ALPHA_SMOKE_DIR:-}" ]]; then
  workdir="$BINDHUB_ALPHA_SMOKE_DIR"
  mkdir -p "$workdir"
  cleanup=false
else
  workdir="$(mktemp -d "${TMPDIR:-/tmp}/bindhub-alpha-smoke.XXXXXX")"
  cleanup=true
fi

if [[ "${BINDHUB_KEEP_SMOKE_DIR:-false}" == "true" ]]; then
  cleanup=false
fi

if [[ "$cleanup" == "true" ]]; then
  trap 'rm -rf "$workdir"' EXIT
fi

evidence_dir="$workdir/evidence"
mkdir -p "$evidence_dir"

if [[ -n "${BINDHUB_BIN:-}" ]]; then
  bindhub_cmd=("$BINDHUB_BIN")
elif [[ -x "$repo_root/bindhub" ]]; then
  bindhub_cmd=("$repo_root/bindhub")
else
  bindhub_cmd=(cargo run --quiet -p bindhub-cli --)
fi

if [[ -n "${BINDHUB_DAEMON_BIN:-}" ]]; then
  daemon_cmd=("$BINDHUB_DAEMON_BIN")
elif [[ -x "$repo_root/bindhub-daemon" ]]; then
  daemon_cmd=("$repo_root/bindhub-daemon")
else
  daemon_cmd=(cargo run --quiet -p bindhub-daemon --)
fi

run_bindhub() {
  (cd "$repo_root" && "${bindhub_cmd[@]}" "$@")
}

run_daemon() {
  (cd "$repo_root" && "${daemon_cmd[@]}" "$@")
}

fail() {
  echo "alpha smoke failed: $*" >&2
  echo "workdir=$workdir" >&2
  exit 1
}

line_value() {
  local body="$1"
  local prefix="$2"
  printf '%s\n' "$body" | awk -v prefix="$prefix" 'index($0, prefix) == 1 { print substr($0, length(prefix) + 1); exit }'
}

export_value() {
  local body="$1"
  local name="$2"
  printf '%s\n' "$body" | sed -n "s/^export ${name}='\(.*\)'$/\1/p" | head -n 1
}

redact_log() {
  sed -E \
    -e "s/export (BINDHUB_PAIRING_[A-Z_]+)='[^']+'/export \1='<redacted>'/g" \
    -e "s/Pairing token: Bindhub-pair-v1:[^[:space:]]+/Pairing token: Bindhub-pair-v1:<redacted>/g" \
    -e "s/(Session token export:).*/\1 <redacted>/g"
}

write_log() {
  local name="$1"
  local body="$2"
  printf '%s\n' "$body" | redact_log > "$evidence_dir/$name.log"
}

source_db="$workdir/source.sqlite3"
receiver_db="$workdir/receiver.sqlite3"
metadata_db="$workdir/metadata.sqlite3"
source_cache="$workdir/source-cache"
receiver_cache="$workdir/receiver-cache"
remote_dir="$workdir/remote"
source_project="$workdir/source-project"
receiver_target="$workdir/receiver-project"

mkdir -p "$source_project/src" "$receiver_target" "$source_cache" "$receiver_cache" "$remote_dir"
printf 'hello from source device\n' > "$source_project/README.md"
printf 'fn main() { println!("alpha smoke"); }\n' > "$source_project/src/main.rs"

init_out="$(run_bindhub init --db "$source_db" --device-name "Alpha desktop")"
write_log "01-source-init" "$init_out"
account_id="$(line_value "$init_out" "Account id: ")"
[[ -n "$account_id" ]] || fail "could not parse source account id"

invite_out="$(run_bindhub devices invite --db "$source_db")"
write_log "02-source-invite" "$invite_out"
pairing_token="$(export_value "$invite_out" "BINDHUB_PAIRING_TOKEN")"
[[ -n "$pairing_token" ]] || fail "could not parse pairing token"

export BINDHUB_PAIRING_TOKEN="$pairing_token"
join_out="$(run_bindhub devices join --db "$receiver_db" --token-env BINDHUB_PAIRING_TOKEN --device-name "Alpha laptop")"
write_log "03-receiver-join" "$join_out"
join_request="$(export_value "$join_out" "BINDHUB_PAIRING_JOIN_REQUEST")"
[[ -n "$join_request" ]] || fail "could not parse receiver join request"

pending_stdout="$evidence_dir/04-pending-receiver.stdout.log"
pending_stderr="$evidence_dir/04-pending-receiver.stderr.log"
if run_daemon sync --db "$receiver_db" --cache "$receiver_cache" --remote "$remote_dir" --push --once "$receiver_target" > "$pending_stdout" 2> "$pending_stderr"; then
  fail "pending receiver unexpectedly synced before devices complete"
fi
if ! grep -q "pending" "$pending_stderr"; then
  fail "pending receiver refusal did not mention pending pairing"
fi

export BINDHUB_PAIRING_JOIN_REQUEST="$join_request"
approve_out="$(run_bindhub devices approve-join --db "$source_db" --token-env BINDHUB_PAIRING_TOKEN --join-request-env BINDHUB_PAIRING_JOIN_REQUEST --device-name "Alpha laptop")"
write_log "05-source-approve-join" "$approve_out"
completion="$(export_value "$approve_out" "BINDHUB_PAIRING_COMPLETION")"
[[ -n "$completion" ]] || fail "could not parse pairing completion"

export BINDHUB_PAIRING_COMPLETION="$completion"
complete_out="$(run_bindhub devices complete --db "$receiver_db" --completion-env BINDHUB_PAIRING_COMPLETION)"
write_log "06-receiver-complete" "$complete_out"
printf '%s\n' "$complete_out" | grep -q "Pairing completed" || fail "receiver did not complete pairing"

push_out="$(run_daemon sync --db "$source_db" --cache "$source_cache" --remote "$remote_dir" --metadata-mode mock-dev-sqlite --metadata-db "$metadata_db" --push --once "$source_project")"
write_log "07-source-live-push" "$push_out"
printf '%s\n' "$push_out" | grep -q "action=publish status=ok" || fail "source live push did not publish"
project_id="$(
  printf '%s\n' "$push_out" |
    awk '/action=publish status=ok/ {
      for (i = 1; i <= NF; i++) {
        if ($i ~ /^project_id=/) {
          sub(/^project_id=/, "", $i)
          print $i
          exit
        }
      }
    }'
)"
[[ -n "$project_id" ]] || fail "could not parse published project id"

pull_out="$(run_daemon sync --db "$receiver_db" --cache "$receiver_cache" --remote "$remote_dir" --metadata-mode mock-dev-sqlite --metadata-db "$metadata_db" --metadata-account "$account_id" --metadata-project "$project_id" --pull --to "$receiver_target" --apply --once "$receiver_target")"
write_log "08-receiver-live-pull" "$pull_out"
printf '%s\n' "$pull_out" | grep -q "action=materialize status=ok" || fail "receiver pull did not materialize"
printf '%s\n' "$pull_out" | grep -q "applied=true" || fail "receiver pull did not apply"

cmp "$source_project/README.md" "$receiver_target/README.md" >/dev/null || fail "README did not materialize"
cmp "$source_project/src/main.rs" "$receiver_target/src/main.rs" >/dev/null || fail "source file did not materialize"

cat > "$evidence_dir/SUMMARY.txt" <<SUMMARY
Bindhub alpha two-device smoke passed.
workdir=$workdir
source_db=$source_db
receiver_db=$receiver_db
metadata_db=$metadata_db
remote_dir=$remote_dir
project_id=$project_id
account_id=$account_id
raw pairing/session/cloud credentials were not written to evidence logs.
SUMMARY

echo "alpha smoke passed"
echo "evidence=$evidence_dir"
echo "source_db=$source_db"
echo "receiver_db=$receiver_db"
echo "metadata_db=$metadata_db"
