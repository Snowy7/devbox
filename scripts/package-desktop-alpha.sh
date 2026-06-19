#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/package-desktop-alpha.sh [VERSION]

Builds a runnable unsigned Electron alpha bundle for the current macOS/Linux host.
The bundle is not a signed installer. Testers extract it, run npm ci, then run
npm run start:built from inside the extracted package.

Environment:
  DEVBOX_DESKTOP_SKIP_NPM_CI=true  Skip npm ci before building.
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="${1:-$(git -C "$repo_root" rev-parse --short HEAD)}"
package_name="devbox-desktop-$version"
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

if [[ "${DEVBOX_DESKTOP_SKIP_NPM_CI:-false}" != "true" ]]; then
  npm ci
fi

npm run test:safety
npm run build

rm -rf "$stage_dir"
mkdir -p "$stage_dir"

cp package.json package-lock.json "$stage_dir/"
cp -R dist "$stage_dir/dist"

cat > "$stage_dir/README.md" <<'README'
# Devbox Desktop Alpha

This is an unsigned alpha Electron control surface. It reads redacted Devbox alpha state from
environment variables and shows commands/state only; it does not start live sync by itself.

## Run

```bash
npm ci
npm run start:built
```

Useful env:

```text
DEVBOX_LIVE_DB
DEVBOX_LIVE_CACHE
DEVBOX_LIVE_PROJECT_ROOT
DEVBOX_REMOTE_KIND
DEVBOX_METADATA_API
DEVBOX_METADATA_PROJECT
DEVBOX_SESSION_TOKEN
```

Source-only commands such as `npm run typecheck`, `npm run test:safety`, and `npm run build` are run
before packaging and are not expected to work from this trimmed release archive.
README

cat > "$stage_dir/RUNNING.txt" <<'RUNNING'
Devbox Desktop Alpha

This is an unsigned alpha Electron control surface. It reads redacted Devbox alpha
state from environment variables and shows commands/state only; it does not start
live sync by itself.

Run:
  npm ci
  npm run start:built

Useful env:
  DEVBOX_LIVE_DB
  DEVBOX_LIVE_CACHE
  DEVBOX_LIVE_PROJECT_ROOT
  DEVBOX_REMOTE_KIND
  DEVBOX_METADATA_API
  DEVBOX_METADATA_PROJECT
  DEVBOX_SESSION_TOKEN

Source-only commands such as npm run typecheck, npm run test:safety, and npm run
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
