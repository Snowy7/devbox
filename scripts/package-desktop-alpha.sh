#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/package-desktop-alpha.sh [VERSION]

Builds a runnable unsigned Electron alpha bundle for the current macOS/Linux host.
The bundle is not a signed installer. Testers extract it, run pnpm install, then run
pnpm start:built from inside the extracted package.

Environment:
  BINDHUB_DESKTOP_SKIP_INSTALL=true  Skip pnpm install before building.
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="${1:-$(git -C "$repo_root" rev-parse --short HEAD)}"
package_name="bindhub-desktop-$version"
dist_dir="$repo_root/dist"
stage_dir="$dist_dir/$package_name"
archive="$dist_dir/$package_name.tar.gz"
checksum="$archive.sha256"

case "$(uname -s)" in
  Linux|Darwin)
    ;;
  *)
    echo "desktop alpha packaging script supports macOS/Linux hosts" >&2
    exit 1
    ;;
esac

cd "$repo_root/apps/desktop"

if [[ "${BINDHUB_DESKTOP_SKIP_INSTALL:-false}" != "true" ]]; then
  pnpm install
fi

pnpm test:safety
pnpm build

rm -rf "$stage_dir"
mkdir -p "$stage_dir"

cp package.json "$stage_dir/"
cp -R dist "$stage_dir/dist"

cat > "$stage_dir/README.md" <<'README'
# Bindhub Desktop Alpha

This is an unsigned alpha Electron control surface. It reads redacted Bindhub alpha state from
environment variables and shows commands/state only; it does not start live sync by itself.

## Run

```bash
pnpm install
pnpm start:built
```

Useful env:

```text
BINDHUB_LIVE_DB
BINDHUB_LIVE_CACHE
BINDHUB_LIVE_PROJECT_ROOT
BINDHUB_REMOTE_KIND
BINDHUB_METADATA_API
BINDHUB_METADATA_PROJECT
BINDHUB_SESSION_TOKEN
```

Source-only commands such as `pnpm typecheck`, `pnpm test:safety`, and `pnpm build` are run
before packaging and are not expected to work from this trimmed release archive.
README

cat > "$stage_dir/RUNNING.txt" <<'RUNNING'
Bindhub Desktop Alpha

This is an unsigned alpha Electron control surface. It reads redacted Bindhub alpha
state from environment variables and shows commands/state only; it does not start
live sync by itself.

Run:
  pnpm install
  pnpm start:built

Useful env:
  BINDHUB_LIVE_DB
  BINDHUB_LIVE_CACHE
  BINDHUB_LIVE_PROJECT_ROOT
  BINDHUB_REMOTE_KIND
  BINDHUB_METADATA_API
  BINDHUB_METADATA_PROJECT
  BINDHUB_SESSION_TOKEN

Source-only commands such as pnpm typecheck, pnpm test:safety, and pnpm
build are run before packaging and are not expected to work from this trimmed
release archive.
RUNNING

tar -czf "$archive" -C "$dist_dir" "$package_name"

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$dist_dir" && sha256sum "$(basename "$archive")" > "$(basename "$checksum")")
else
  hash="$(shasum -a 256 "$archive" | awk '{print $1}')"
  printf "%s  %s\n" "$hash" "$(basename "$archive")" > "$checksum"
fi

echo "archive=$archive"
echo "checksum=$checksum"
