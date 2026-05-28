#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

refspec_file="${1:-}"
tmp_file=""

if [[ -z "$refspec_file" ]]; then
  tmp_file="$(mktemp)"
  cat >"$tmp_file"
  refspec_file="$tmp_file"
fi

cleanup() {
  if [[ -n "$tmp_file" && -f "$tmp_file" ]]; then
    rm -f "$tmp_file"
  fi
}
trap cleanup EXIT

if [[ ! -f "$refspec_file" || ! -s "$refspec_file" ]]; then
  exit 0
fi

if [[ ! -f CHANGELOG.md ]]; then
  echo "Blocked pre-push: CHANGELOG.md is missing." >&2
  echo "Run: just changelog <tag>" >&2
  exit 1
fi

missing_tags=()

while read -r local_ref local_sha remote_ref remote_sha; do
  [[ -z "${local_ref:-}" ]] && continue
  [[ "$local_ref" != refs/tags/* ]] && continue

  # Skip tag deletion pushes.
  if [[ "${local_sha:-}" =~ ^0+$ ]]; then
    continue
  fi

  tag="${local_ref#refs/tags/}"
  if ! grep -Fq "## [${tag}] - " CHANGELOG.md; then
    missing_tags+=("$tag")
  fi
done <"$refspec_file"

if [[ ${#missing_tags[@]} -gt 0 ]]; then
  {
    echo "Blocked pre-push: missing CHANGELOG entries for pushed tag(s):"
    for tag in "${missing_tags[@]}"; do
      echo "  - ${tag}  (run: just changelog ${tag})"
    done
  } >&2
  exit 1
fi

