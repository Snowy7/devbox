#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/publish-cli-release.sh <TAG>

Builds the current macOS/Linux host CLI package, creates the tag if needed,
pushes the tag, and uploads the package to a GitHub Release.

Examples:
  scripts/publish-cli-release.sh v0.1.0-alpha.1
  DEVBOX_RELEASE_TARGET=aarch64-apple-darwin scripts/publish-cli-release.sh v0.1.0-alpha.1
USAGE
}

tag="${1:-}"
if [[ "$tag" == "-h" || "$tag" == "--help" ]]; then
  usage
  exit 0
fi

if [[ -z "$tag" ]]; then
  usage
  exit 1
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if ! command -v gh >/dev/null 2>&1; then
  echo "gh is required to upload the release" >&2
  exit 1
fi

gh auth status >/dev/null

if [[ -n "$(git status --porcelain)" ]]; then
  echo "working tree has uncommitted changes; commit before publishing a release" >&2
  exit 1
fi

if ! git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  git tag "$tag"
fi

git push origin "$tag"

"$repo_root/scripts/package-cli.sh" "$tag"

mapfile -t assets < <(find "$repo_root/dist" -maxdepth 1 -type f \( -name "devbox-$tag-*.tar.gz" -o -name "devbox-$tag-*.tar.gz.sha256" \) | sort)
if [[ "${#assets[@]}" -eq 0 ]]; then
  echo "no release assets found for $tag" >&2
  exit 1
fi

repo="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"
release_flags=()
if [[ "$tag" == *-* ]]; then
  release_flags+=(--prerelease)
fi

if gh release view "$tag" --repo "$repo" >/dev/null 2>&1; then
  gh release upload "$tag" "${assets[@]}" --repo "$repo" --clobber
else
  gh release create "$tag" "${assets[@]}" \
    --repo "$repo" \
    --title "Devbox CLI $tag" \
    --notes "Alpha command-line tools for manual R2, hosted metadata, and two-device live-sync testing. OAuth, signed installers, and object proxy transfer are not included yet." \
    "${release_flags[@]}"
fi

echo "published assets for $tag"
