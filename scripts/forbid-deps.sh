#!/usr/bin/env bash
# Fail if forbidden crates appear under [dependencies].
set -euo pipefail

echo "PLAN: scan Cargo.toml [dependencies] for forbidden crates"

toml="${1:-Cargo.toml}"
if [[ ! -f "$toml" ]]; then
  echo "FAIL: missing $toml"
  echo "DONE: ok=false"
  exit 1
fi

echo "DO: awk [dependencies] block"
deps="$(awk '
  /^\[dependencies\]/ { p=1; next }
  /^\[/ { p=0 }
  p { print }
' "$toml")"

if printf '%s\n' "$deps" | grep -E '^(bline-|canact|wiremux|wiremux-auth|typesafe-rs|jevrs)'; then
  echo "FAIL: forbidden crate in [dependencies]"
  echo "DONE: ok=false"
  exit 1
fi

echo "OK: no forbidden crates"
echo "DONE: ok=true"
echo "NEXT: keep [dependencies] free of bline/canact/wiremux"
