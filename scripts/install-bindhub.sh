#!/usr/bin/env sh
set -eu

repo="${BINDHUB_REPO:-Snowy7/devbox}"
version="${BINDHUB_VERSION:-${1:-latest}}"
install_dir="${BINDHUB_INSTALL_DIR:-$HOME/.local/bin}"

detect_target() {
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os:$arch" in
    Darwin:arm64) echo "aarch64-apple-darwin" ;;
    Darwin:x86_64) echo "x86_64-apple-darwin" ;;
    Linux:x86_64) echo "x86_64-unknown-linux-gnu" ;;
    *) echo "unsupported platform: $os $arch" >&2; exit 1 ;;
  esac
}

latest_tag() {
  curl_github "https://api.github.com/repos/$repo/releases" \
    | sed -n 's/.*"tag_name": "\(v[^"]*\)".*/\1/p' \
    | head -n 1
}

curl_github() {
  token="${BINDHUB_GITHUB_TOKEN:-${GITHUB_TOKEN:-}}"
  if [ -n "$token" ]; then
    curl -fsSL -H "Authorization: Bearer $token" -H "Accept: application/vnd.github+json" "$@"
  else
    curl -fsSL -H "Accept: application/vnd.github+json" "$@"
  fi
}

curl_github_asset() {
  token="${BINDHUB_GITHUB_TOKEN:-${GITHUB_TOKEN:-}}"
  if [ -n "$token" ]; then
    curl -fsSL -H "Authorization: Bearer $token" -H "Accept: application/octet-stream" "$@"
  else
    curl -fsSL -H "Accept: application/octet-stream" "$@"
  fi
}

asset_url() {
  asset_name="$1"
  curl_github "https://api.github.com/repos/$repo/releases/tags/$tag" \
    | awk -v name="$asset_name" '
      /"url":/ {
        url=$0
        sub(/^[[:space:]]*"url": "/, "", url)
        sub(/",?$/, "", url)
      }
      /"name":/ && index($0, "\"" name "\"") {
        print url
        exit
      }
    '
}

add_path_hint() {
  case ":$PATH:" in
    *":$install_dir:"*) return ;;
  esac

  shell_name="$(basename "${SHELL:-sh}")"
  case "$shell_name" in
    zsh) rc_file="$HOME/.zshrc" ;;
    bash) rc_file="$HOME/.bashrc" ;;
    *) rc_file="$HOME/.profile" ;;
  esac

  marker="# Bindhub CLI"
  if [ ! -f "$rc_file" ] || ! grep -F "$marker" "$rc_file" >/dev/null 2>&1; then
    {
      printf '\n%s\n' "$marker"
      printf 'export PATH="%s:$PATH"\n' "$install_dir"
    } >> "$rc_file"
  fi
}

target="$(detect_target)"
if [ "$version" = "latest" ]; then
  tag="$(latest_tag)"
else
  tag="$version"
fi

if [ -z "$tag" ]; then
  echo "No Bindhub releases found for $repo" >&2
  exit 1
fi

asset="bindhub-$tag-$target.tar.gz"
asset_api_url="$(asset_url "$asset")"
checksum_api_url="$(asset_url "$asset.sha256")"
if [ -z "$asset_api_url" ] || [ -z "$checksum_api_url" ]; then
  echo "Release $tag does not have $asset and checksum assets" >&2
  exit 1
fi
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/bindhub-install.XXXXXX")"

cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

curl_github_asset "$asset_api_url" -o "$tmp_dir/$asset"
curl_github_asset "$checksum_api_url" -o "$tmp_dir/$asset.sha256"

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$tmp_dir" && sha256sum -c "$asset.sha256")
else
  expected="$(awk '{print $1}' "$tmp_dir/$asset.sha256")"
  actual="$(shasum -a 256 "$tmp_dir/$asset" | awk '{print $1}')"
  if [ "$expected" != "$actual" ]; then
    echo "Checksum mismatch for $asset" >&2
    exit 1
  fi
fi

tar -xzf "$tmp_dir/$asset" -C "$tmp_dir"
package_dir="$tmp_dir/bindhub-$tag-$target"
mkdir -p "$install_dir"

for binary in loom bindhub bindhub-daemon bindhub-metadata; do
  cp "$package_dir/$binary" "$install_dir/$binary"
  chmod +x "$install_dir/$binary"
done

if [ "$(uname -s)" = "Darwin" ] && command -v xattr >/dev/null 2>&1; then
  xattr -dr com.apple.quarantine \
    "$install_dir/loom" \
    "$install_dir/bindhub" \
    "$install_dir/bindhub-daemon" \
    "$install_dir/bindhub-metadata" 2>/dev/null || true
fi

if [ "${BINDHUB_NO_PATH:-}" != "1" ]; then
  add_path_hint
fi

echo "Bindhub $tag installed to $install_dir"
if [ "${BINDHUB_NO_PATH:-}" = "1" ]; then
  echo "PATH was not changed because BINDHUB_NO_PATH=1 was set."
else
  echo "Open a new terminal, then run: bindhub --help"
fi
echo "To update later, rerun this script."
