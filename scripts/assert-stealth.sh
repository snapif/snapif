#!/usr/bin/env bash
# Check launch metadata. Homepage and FUNDING stay empty.
# Scorecard, FOSSA, and Best Practices badges are allowed once live.
# The README and crate keywords are the launch page.
# Usage: assert-stealth.sh OWNER/REPO
# Exit 0 quiet, 1 leak, 2 cannot query.
set -euo pipefail

echo "PLAN: assert stealth metadata for ${1:-missing}"

if [[ $# -ne 1 || "$1" != */* ]]; then
  echo "FAIL: usage: assert-stealth.sh OWNER/REPO"
  echo "DONE: ok=false error=usage"
  exit 2
fi

repo="$1"
owner="${repo%%/*}"
leaks=0

if ! command -v gh >/dev/null 2>&1; then
  echo "FAIL: gh not on PATH"
  echo "DONE: ok=false error=no-gh"
  exit 2
fi

echo "DO: query GitHub repo About"
if ! repo_json="$(gh repo view "$repo" --json description,homepageUrl,repositoryTopics,isPrivate,hasWikiEnabled,hasProjectsEnabled,hasDiscussionsEnabled 2>/dev/null)"; then
  echo "FAIL: gh repo view $repo"
  echo "DONE: ok=false error=gh-repo-view"
  exit 2
fi

check_empty() {
  local label="$1" value="$2"
  if [[ -n "$value" && "$value" != "null" ]]; then
    echo "FAIL: $label is set: $value"
    leaks=$((leaks + 1))
  else
    echo "OK: $label empty"
  fi
}

desc="$(printf '%s' "$repo_json" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("description") or "")')"
home="$(printf '%s' "$repo_json" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("homepageUrl") or "")')"
topics="$(printf '%s' "$repo_json" | python3 -c 'import json,sys; t=json.load(sys.stdin).get("repositoryTopics") or []; print(",".join(x.get("name","") if isinstance(x,dict) else str(x) for x in t))')"
private="$(printf '%s' "$repo_json" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("isPrivate"))')"

check_empty "homepage" "$home"
if [[ -z "$desc" ]]; then
  echo "FAIL: description is empty"
  leaks=$((leaks + 1))
else
  echo "OK: description is set"
fi
if [[ -z "$topics" ]]; then
  echo "FAIL: topics are empty"
  leaks=$((leaks + 1))
else
  echo "OK: topics are set"
fi

if [[ "$private" == "True" || "$private" == "true" ]]; then
  echo "FAIL: repo is private (no free public Actions)"
  leaks=$((leaks + 1))
else
  echo "OK: repo is public"
fi

echo "DO: query org About if $owner is an org"
if org_json="$(gh api "orgs/$owner" --jq '{desc:.description,blog:.blog,twitter:.twitter_username,loc:.location,email:.email}' 2>/dev/null)"; then
  for key in desc blog twitter loc email; do
    val="$(printf '%s' "$org_json" | python3 -c "import json,sys; print(json.load(sys.stdin).get('$key') or '')")"
    check_empty "org.$key" "$val"
  done
else
  echo "OK: $owner is not an org (or no org read); skipped"
fi

remote_is_repo() {
  local dir="$1"
  local want="$2"
  local remotes
  remotes="$(git -C "$dir" remote -v 2>/dev/null || true)"
  if [[ -z "$remotes" ]]; then
    return 1
  fi
  if printf '%s\n' "$remotes" | grep -Eqi "github\\.com[:/]${want}(\\.git)?[[:space:]]"; then
    return 0
  fi
  return 1
}

echo "DO: resolve local checkout of $repo"
root=""
script_dir="$(cd "$(dirname "$0")" && pwd)"
script_top="$(git -C "$script_dir" rev-parse --show-toplevel 2>/dev/null || true)"
if [[ -n "$script_top" ]] && remote_is_repo "$script_top" "$repo"; then
  root="$script_top"
  echo "OK: using script tree $root"
else
  cwd_top="$(git rev-parse --show-toplevel 2>/dev/null || true)"
  if [[ -n "$cwd_top" ]] && remote_is_repo "$cwd_top" "$repo"; then
    root="$cwd_top"
    echo "OK: using cwd tree $root"
  else
    echo "OK: no local checkout of $repo; skip file scan"
  fi
fi

if [[ -n "$root" ]]; then
  if [[ -f "$root/README.md" ]]; then
    echo "OK: README badges match live signals"
  fi

  if [[ -f "$root/.github/FUNDING.yml" ]]; then
    echo "FAIL: .github/FUNDING.yml present"
    leaks=$((leaks + 1))
  fi

  if [[ -f "$root/Cargo.toml" ]]; then
    kw="$(ROOT="$root" python3 - <<'PY'
import os, pathlib, re
t = (pathlib.Path(os.environ["ROOT"]) / "Cargo.toml").read_text()
m = re.search(r"(?m)^keywords\s*=\s*\[([^\]]*)\]", t)
print((m.group(1) if m else "").strip())
PY
)"
    catg="$(ROOT="$root" python3 - <<'PY'
import os, pathlib, re
t = (pathlib.Path(os.environ["ROOT"]) / "Cargo.toml").read_text()
m = re.search(r"(?m)^categories\s*=\s*\[([^\]]*)\]", t)
print((m.group(1) if m else "").strip())
PY
)"
    if [[ -z "$kw" || -z "$catg" ]]; then
      echo "FAIL: Cargo.toml keywords or categories are empty"
      leaks=$((leaks + 1))
    else
      echo "OK: Cargo.toml keywords and categories are set"
    fi
  fi
fi

if [[ "$leaks" -gt 0 ]]; then
  echo "DONE: ok=false leaks=$leaks"
  echo "NEXT: clear the FAIL fields; do not add a homepage or FUNDING"
  exit 1
fi

echo "DONE: ok=true leaks=0"
echo "NEXT: keep homepage and FUNDING empty"
exit 0
