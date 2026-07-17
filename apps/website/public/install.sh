#!/bin/sh
set -eu

DOWNLOAD_BASE_URL="${DOWNLOAD_BASE_URL:-https://assets.lawlint.com/downloads}"
DOWNLOAD_BASE_URL="${DOWNLOAD_BASE_URL%/}"

os="$(uname -s)"
arch="$(uname -m)"
case "$os:$arch" in
  Darwin:arm64|Darwin:aarch64) target="aarch64-apple-darwin" ;;
  Darwin:x86_64) target="x86_64-apple-darwin" ;;
  Linux:x86_64|Linux:amd64) target="x86_64-unknown-linux-gnu" ;;
  *) echo "lawlint does not currently publish a CLI for $os/$arch." >&2; exit 1 ;;
esac

archive="lawlint-$target.tar.gz"
url="$DOWNLOAD_BASE_URL/latest/$archive"
tmp_dir="$(mktemp -d 2>/dev/null || mktemp -d -t lawlint)"
trap 'rm -rf "$tmp_dir"' EXIT INT TERM

echo "Downloading lawlint for $target..."
curl --fail --location --silent --show-error "$url" --output "$tmp_dir/$archive"
tar -xzf "$tmp_dir/$archive" -C "$tmp_dir"

install_dir="${LAWLINT_INSTALL_DIR:-$HOME/.local/bin}"
if [ ! -d "$install_dir" ]; then
  mkdir -p "$install_dir" 2>/dev/null || true
fi

if [ -w "$install_dir" ]; then
  cp "$tmp_dir/lawlint" "$install_dir/lawlint"
  chmod 755 "$install_dir/lawlint"
else
  install_dir="/usr/local/bin"
  echo "Installing to $install_dir requires administrator access."
  sudo install -m 755 "$tmp_dir/lawlint" "$install_dir/lawlint"
fi

echo "Installed lawlint to $install_dir/lawlint"
case ":${PATH:-}:" in
  *:"$install_dir":*) ;;
  *) echo "Add $install_dir to PATH, then run: lawlint --help" ;;
esac
