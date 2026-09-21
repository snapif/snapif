#!/usr/bin/env bash
# Fail if forbidden crates appear under [dependencies] or [dependencies.*].
set -euo pipefail

echo "PLAN: scan Cargo.toml [dependencies] and [dependencies.*] for forbidden crates"

toml="${1:-Cargo.toml}"
if [[ ! -f "$toml" ]]; then
  echo "FAIL: missing $toml"
  echo "DONE: ok=false"
  exit 1
fi

forbidden_key='^(bline-|canact|wiremux|wiremux-auth|typesafe-rs|jevrs)'
forbidden_table='^\[dependencies\.(bline-[^]]*|canact|wiremux|wiremux-auth|typesafe-rs|jevrs)\]'

echo "DO: named [dependencies.<crate>] tables"
if grep -E "$forbidden_table" "$toml"; then
  echo "FAIL: forbidden crate as [dependencies.<name>] table"
  echo "DONE: ok=false"
  exit 1
fi

echo "DO: keys in [dependencies] and [dependencies.*] tables"
deps="$(awk '
  /^\[dependencies\]/ { p=1; next }
  /^\[dependencies\./ { p=1; next }
  /^\[/ { p=0 }
  p { print }
' "$toml")"

if printf '%s\n' "$deps" | grep -E "$forbidden_key"; then
  echo "FAIL: forbidden crate in a dependencies table"
  echo "DONE: ok=false"
  exit 1
fi

echo "OK: no forbidden crates"
echo "DONE: ok=true"
echo "NEXT: keep [dependencies] and [dependencies.*] free of bline/canact/wiremux"
