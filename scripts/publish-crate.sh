#!/usr/bin/env bash
# Publish the checked-out snapif crate when that version is not on crates.io.
# CARGO_REGISTRY_TOKEN must already be an OIDC token or the repo fallback.
set -euo pipefail

if [ -z "${CARGO_REGISTRY_TOKEN:-}" ]; then
  echo "crates.io token is empty" >&2
  exit 1
fi

version=$(awk '
  $0 == "[package]" { in_pkg = 1; next }
  /^\[/ { in_pkg = 0 }
  in_pkg && $1 == "version" {
    gsub(/"/, "", $3)
    print $3
    exit
  }
' Cargo.toml)

if ! printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
  echo "Cargo.toml version is not X.Y.Z: ${version}" >&2
  exit 1
fi

code=$(curl --retry 5 --retry-delay 2 -sS -A "snapif-release" -o /tmp/snapif-crate-lookup.json -w "%{http_code}" \
  "https://crates.io/api/v1/crates/snapif/${version}")
case "$code" in
  200)
    echo "snapif ${version} is already on crates.io"
    exit 0
    ;;
  404)
    ;;
  *)
    echo "crates.io lookup for snapif ${version} returned ${code}" >&2
    exit 1
    ;;
esac

cargo publish --locked
echo "published snapif ${version}"
