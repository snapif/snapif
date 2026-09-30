#!/usr/bin/env bash
# Attach Cosign bundles and GitHub attestation files to a GitHub Release.
#
# Scorecard Signed-Releases reads Release assets, not the Attestations
# API. Upload .sigstore.json (Cosign keyless) and .intoto.jsonl
# (SLSA provenance). Do not upload .sigstore.jsonl.
#
# Required environment:
#   TAG         Release tag (snapif-vX.Y.Z)
#   REPO        owner/name
#   ARTIFACTS   directory of assets to sign
# Optional:
#   DRY_RUN=1   print subjects and exit (no cosign, no upload)
#   GH_TOKEN    needed unless DRY_RUN=1
set -euo pipefail

is_signature_name() {
  case "$1" in
    *.sigstore.json | *.sigstore.jsonl | *.intoto.jsonl | *.sig | *.asc) return 0 ;;
    *) return 1 ;;
  esac
}

require_env() {
  local name="$1"
  if [ -z "${!name:-}" ]; then
    echo "FAIL: ${name} is required" >&2
    exit 1
  fi
}

require_env TAG
require_env REPO
require_env ARTIFACTS

if ! printf '%s\n' "$TAG" | grep -Eq '^snapif-v[0-9]+\.[0-9]+\.[0-9]+$'; then
  echo "FAIL: TAG must look like snapif-vX.Y.Z: ${TAG}" >&2
  exit 1
fi

if [ ! -d "$ARTIFACTS" ]; then
  echo "FAIL: ARTIFACTS is not a directory: ${ARTIFACTS}" >&2
  exit 1
fi
# Later we cd into a temp dir for gh attestation download.
# Resolve now so those paths stay valid.
ARTIFACTS=$(cd "$ARTIFACTS" && pwd)

subjects=()
while IFS= read -r -d '' path; do
  name=$(basename "$path")
  if is_signature_name "$name"; then
    continue
  fi
  subjects+=("$path")
done < <(find "$ARTIFACTS" -maxdepth 1 -type f -print0)
if [ "${#subjects[@]}" -gt 0 ]; then
  sorted=()
  while IFS= read -r path; do
    sorted+=("$path")
  done < <(printf '%s\n' "${subjects[@]}" | LC_ALL=C sort)
  subjects=("${sorted[@]}")
fi

if [ "${#subjects[@]}" -eq 0 ]; then
  echo "FAIL: no signable assets in ${ARTIFACTS}" >&2
  exit 1
fi

echo "PLAN: sign ${#subjects[@]} assets for ${TAG} in ${REPO} from ${ARTIFACTS}"
for path in "${subjects[@]}"; do
  echo "SUBJECT: $(basename "$path")"
done

if [ "${DRY_RUN:-}" = "1" ]; then
  echo "OK: dry-run listed ${#subjects[@]} subjects"
  echo "DONE: dry-run"
  exit 0
fi

if ! command -v cosign >/dev/null 2>&1; then
  echo "FAIL: cosign is not on PATH" >&2
  exit 1
fi
if ! command -v gh >/dev/null 2>&1; then
  echo "FAIL: gh is not on PATH" >&2
  exit 1
fi
require_env GH_TOKEN

workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT
mkdir -p "${workdir}/bundles"

echo "DO: Cosign sign-blob"
for path in "${subjects[@]}"; do
  name=$(basename "$path")
  echo "DO: cosign ${name}"
  cosign sign-blob --yes --bundle "${workdir}/bundles/${name}.sigstore.json" "$path"
done

echo "DO: download attestation bundles"
for path in "${subjects[@]}"; do
  name=$(basename "$path")
  tmpdir=$(mktemp -d)
  download_ok=0
  pushd "$tmpdir" >/dev/null
  for _attempt in 1 2 3 4 5; do
    if gh attestation download "$path" --repo "$REPO"; then
      download_ok=1
      break
    fi
    sleep 2
  done
  if [ "$download_ok" -eq 0 ]; then
    popd >/dev/null
    rm -rf "$tmpdir"
    echo "FAIL: gh attestation download ${name}" >&2
    exit 1
  fi
  found=0
  for bundle in *.jsonl; do
    if [ -f "$bundle" ]; then
      cp "$bundle" "${workdir}/bundles/${name}.intoto.jsonl"
      found=1
      break
    fi
  done
  popd >/dev/null
  rm -rf "$tmpdir"
  if [ "$found" -eq 0 ]; then
    echo "FAIL: no .jsonl from gh attestation download for ${name}" >&2
    exit 1
  fi
done

echo "DO: upload signature assets to ${TAG}"
gh release upload "$TAG" "${workdir}/bundles"/* --repo "$REPO" --clobber
echo "OK: uploaded Cosign and provenance assets for ${TAG}"
echo "DONE: signed ${#subjects[@]} assets"
