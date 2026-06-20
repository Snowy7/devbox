#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/package-cli.sh [VERSION]

Builds and packages Devbox alpha command-line tools for the current macOS/Linux host.

Environment:
  DEVBOX_RELEASE_TARGET   Optional Rust target triple.
  DEVBOX_DEFAULT_API_URL  Optional default Devbox API URL baked into the CLI.

Examples:
  scripts/package-cli.sh v0.1.0-alpha.1
  DEVBOX_RELEASE_TARGET=x86_64-unknown-linux-gnu scripts/package-cli.sh v0.1.0-alpha.1
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

version="${1:-$(git rev-parse --short HEAD)}"
target="${DEVBOX_RELEASE_TARGET:-}"
export DEVBOX_DEFAULT_API_URL="${DEVBOX_DEFAULT_API_URL:-https://devbox-staging.up.railway.app}"

if [[ -z "$target" ]]; then
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os:$arch" in
    Linux:x86_64)
      target="x86_64-unknown-linux-gnu"
      ;;
    Darwin:arm64)
      target="aarch64-apple-darwin"
      ;;
    Darwin:x86_64)
      target="x86_64-apple-darwin"
      ;;
    *)
      echo "unsupported host for default target: $os $arch" >&2
      echo "set DEVBOX_RELEASE_TARGET explicitly if Rust can build it here" >&2
      exit 1
      ;;
  esac
fi

case "$target" in
  x86_64-unknown-linux-gnu|aarch64-apple-darwin|x86_64-apple-darwin)
    ;;
  *)
    echo "local bash packaging currently supports Linux/macOS targets only: $target" >&2
    exit 1
    ;;
esac

package_name="devbox-$version-$target"
dist_dir="$repo_root/dist"
stage_dir="$dist_dir/$package_name"
archive="$dist_dir/$package_name.tar.gz"
checksum="$archive.sha256"

rm -rf "$stage_dir"
mkdir -p "$stage_dir"

rustup target add "$target"
cargo build --release --locked \
  -p loom-cli \
  -p devbox-cli \
  -p devbox-daemon \
  -p devbox-metadata \
  --target "$target"

cp "$repo_root/target/$target/release/loom" "$stage_dir/loom"
cp "$repo_root/target/$target/release/devbox" "$stage_dir/devbox"
cp "$repo_root/target/$target/release/devbox-daemon" "$stage_dir/devbox-daemon"
cp "$repo_root/target/$target/release/devbox-metadata" "$stage_dir/devbox-metadata"
cp "$repo_root/README.md" "$stage_dir/README.md"
cp "$repo_root/LICENSE" "$stage_dir/LICENSE"
cat > "$stage_dir/.env.example" <<'ENV'
# Devbox CLI local/dev overrides.
# Packaged production builds should already know the Devbox API endpoint.

# DEVBOX_API_URL=https://devbox-staging.up.railway.app
DEVBOX_CONFIG_DIR=.devbox
ENV
cp "$repo_root/.env.example" "$stage_dir/.env.operator.example"
mkdir -p "$stage_dir/scripts" "$stage_dir/docs"
cp "$repo_root/scripts/install-devbox.sh" "$stage_dir/scripts/install-devbox.sh"
cp "$repo_root/scripts/install-devbox.ps1" "$stage_dir/scripts/install-devbox.ps1"
cp "$repo_root/scripts/load-r2-env.sh" "$stage_dir/scripts/load-r2-env.sh"
cp "$repo_root/scripts/devbox-live-sync-alpha.sh" "$stage_dir/scripts/devbox-live-sync-alpha.sh"
cp "$repo_root/scripts/alpha-two-device-smoke.sh" "$stage_dir/scripts/alpha-two-device-smoke.sh"
cp "$repo_root/docs/alpha-cli-distribution.md" "$stage_dir/docs/alpha-cli-distribution.md"

tar -czf "$archive" -C "$dist_dir" "$package_name"

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$dist_dir" && sha256sum "$(basename "$archive")" > "$(basename "$checksum")")
else
  hash="$(shasum -a 256 "$archive" | awk '{print $1}')"
  printf "%s  %s\n" "$hash" "$(basename "$archive")" > "$checksum"
fi

echo "archive=$archive"
echo "checksum=$checksum"
