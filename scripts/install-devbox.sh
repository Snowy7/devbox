#!/usr/bin/env sh
set -eu

repo="${DEVBOX_REPO:-Snowy7/devbox}"
version="${DEVBOX_VERSION:-${1:-latest}}"
install_dir="${DEVBOX_INSTALL_DIR:-$HOME/.local/bin}"

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
  curl -fsSL "https://api.github.com/repos/$repo/releases" \
    | sed -n 's/.*"tag_name": "\(v[^"]*\)".*/\1/p' \
    | head -n 1
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

  marker="# Devbox CLI"
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
  echo "No Devbox releases found for $repo" >&2
  exit 1
fi

asset="devbox-$tag-$target.tar.gz"
base_url="https://github.com/$repo/releases/download/$tag"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/devbox-install.XXXXXX")"

cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

curl -fL "$base_url/$asset" -o "$tmp_dir/$asset"
curl -fL "$base_url/$asset.sha256" -o "$tmp_dir/$asset.sha256"

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
package_dir="$tmp_dir/devbox-$tag-$target"
mkdir -p "$install_dir"

for binary in loom devbox devbox-daemon devbox-metadata; do
  cp "$package_dir/$binary" "$install_dir/$binary"
  chmod +x "$install_dir/$binary"
done

if [ "$(uname -s)" = "Darwin" ] && command -v xattr >/dev/null 2>&1; then
  xattr -dr com.apple.quarantine \
    "$install_dir/loom" \
    "$install_dir/devbox" \
    "$install_dir/devbox-daemon" \
    "$install_dir/devbox-metadata" 2>/dev/null || true
fi

add_path_hint

echo "Devbox $tag installed to $install_dir"
echo "Open a new terminal, then run: devbox --help"
echo "To update later, rerun this script."
