#!/usr/bin/env bash
# Install cargo-dist 0.31.0 from a checksummed archive.
# The version matches dist-workspace.toml. Do not curl | sh the installer.
set -euo pipefail

DIST_VERSION=v0.31.0

case "$(uname -s)" in
  Linux)
    os=unknown-linux-gnu
    ext=tar.xz
    ;;
  Darwin)
    os=apple-darwin
    ext=tar.xz
    ;;
  MINGW* | MSYS* | CYGWIN*)
    os=pc-windows-msvc
    ext=zip
    ;;
  *)
    echo "unsupported OS $(uname -s)" >&2
    exit 1
    ;;
esac

case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *)
    echo "unsupported arch $(uname -m)" >&2
    exit 1
    ;;
esac

archive="cargo-dist-${arch}-${os}.${ext}"
case "$archive" in
  cargo-dist-x86_64-unknown-linux-gnu.tar.xz)
    sha=cd355dab0b4c02fb59038fef87655550021d07f45f1d82f947a34ef98560abb8
    ;;
  cargo-dist-aarch64-unknown-linux-gnu.tar.xz)
    sha=382cc29ff91ef12a5bf78ad8ee1804661d24e2fbe64b1bdedd6078723b677ae5
    ;;
  cargo-dist-x86_64-apple-darwin.tar.xz)
    sha=fd4d8f9f07802359cbcdc52bac3abd7d5201c4b73a7cbcdd6faca2232a389f0c
    ;;
  cargo-dist-aarch64-apple-darwin.tar.xz)
    sha=decb01c64c12501931c3cac3111b368a7f48adf8d9e65455c08e5757b9a1fd6f
    ;;
  cargo-dist-x86_64-pc-windows-msvc.zip)
    sha=a14e17557b269b101405e0cc6b647581d56313c954a51c7fddd423bba21e17b2
    ;;
  *)
    echo "no checksum for ${archive}" >&2
    exit 1
    ;;
esac

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
url="https://github.com/axodotdev/cargo-dist/releases/download/${DIST_VERSION}/${archive}"
curl --retry 5 --retry-delay 2 --proto '=https' --tlsv1.2 -LsSf -o "${tmp}/${archive}" "$url"

hash_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
    return
  fi
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
    return
  fi
  local winpath="$1"
  if command -v cygpath >/dev/null 2>&1; then
    winpath=$(cygpath -w "$1")
  fi
  powershell.exe -NoProfile -Command "(Get-FileHash -Algorithm SHA256 -LiteralPath '${winpath}').Hash.ToLower()"
}
actual=$(hash_file "${tmp}/${archive}")
if [ "$actual" != "$sha" ]; then
  echo "checksum mismatch for ${archive}" >&2
  exit 1
fi

pick_one() {
  local found=""
  local candidate
  while IFS= read -r candidate; do
    if [ -z "$found" ]; then
      found=$candidate
    fi
  done
  printf '%s\n' "$found"
}

if [ "$ext" = "zip" ]; then
  unzip -q "${tmp}/${archive}" -d "$tmp"
  bin=$(find "$tmp" -type f -name 'dist.exe' | pick_one)
else
  tar -xJf "${tmp}/${archive}" -C "$tmp"
  bin=$(find "$tmp" -type f -name 'dist' ! -name '*.exe' | pick_one)
fi
if [ -z "${bin}" ]; then
  echo "dist binary missing in ${archive}" >&2
  exit 1
fi

install -d "${HOME}/.cargo/bin"
install -m 755 "$bin" "${HOME}/.cargo/bin/"
if [ -n "${GITHUB_PATH:-}" ]; then
  echo "${HOME}/.cargo/bin" >> "${GITHUB_PATH}"
fi
echo "installed ${archive}"
