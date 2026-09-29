#!/usr/bin/env bash
# Wrapper used by .github/actions/snapif. The binary is snapif on PATH,
# or target/release/snapif, or target/debug/snapif.
set -u
mode="${1:-gate}"
call="${2:-}"
state="${3:-}"
policy="${4:-}"
question="${5:-}"
deny_label="${6:-}"
version="${7:-}"

if [[ -n "$version" ]]; then
  case "$version" in
    *[!0-9A-Za-z.+_-]*)
      echo "snapif version must be a crate version, got $version" >&2
      exit 1
      ;;
  esac
  cargo install snapif --version "$version" --features cli
  export PATH="${HOME}/.cargo/bin:${PATH}"
fi

if [[ -n "${SNAPIF_BIN:-}" ]]; then
  bin="$SNAPIF_BIN"
elif command -v snapif >/dev/null 2>&1; then
  bin="snapif"
elif [[ -x target/release/snapif ]]; then
  bin="target/release/snapif"
elif [[ -x target/debug/snapif ]]; then
  bin="target/debug/snapif"
else
  echo "snapif binary not found" >&2
  exit 1
fi

policy_args=()
if [[ -n "$policy" ]]; then
  policy_args=(--policy "$policy")
fi

case "$mode" in
  gate)
    if [[ -z "$call" ]]; then
      echo "missing call file" >&2
      exit 1
    fi
    "$bin" gate "${policy_args[@]}" --call "$call"
    ;;
  ask)
    if [[ -z "$state" ]]; then
      echo "missing state file" >&2
      exit 1
    fi
    out="$("$bin" ask "${policy_args[@]}" --decisions --state "$state")"
    code=$?
    printf '%s\n' "$out"
    if [[ "$code" -ne 0 ]]; then
      exit "$code"
    fi
    if [[ -n "$question" && -n "$deny_label" ]]; then
      python3 - "$question" "$deny_label" "$out" <<'PY'
import json, sys
question, deny, raw = sys.argv[1:]
body = json.loads(raw)
for row in body["decisions"]:
    if row["id"] != question:
        continue
    known = row.get("known")
    if known is None or str(known).lower() == deny.lower():
        sys.exit(10)
    sys.exit(0)
sys.exit(1)
PY
    fi
    ;;
  *)
    echo "unknown mode $mode" >&2
    exit 1
    ;;
esac
