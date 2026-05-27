#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/generate-changelog.sh <version> [--since <ref>] [--dry-run]

Examples:
  scripts/generate-changelog.sh v0.0.4
  scripts/generate-changelog.sh v0.0.4 --since v0.0.3
  scripts/generate-changelog.sh v0.0.4 --dry-run
EOF
}

if [[ $# -lt 1 ]]; then
  usage
  exit 2
fi

version="$1"
shift

since_ref=""
dry_run="0"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --since)
      shift
      if [[ $# -eq 0 ]]; then
        echo "Missing value for --since" >&2
        exit 2
      fi
      since_ref="$1"
      ;;
    --dry-run)
      dry_run="1"
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 2
      ;;
  esac
  shift
done

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

resolve_github_repo_url() {
  local remote_url
  remote_url="$(git config --get remote.origin.url 2>/dev/null || true)"
  if [[ -z "$remote_url" ]]; then
    return 0
  fi

  if [[ "$remote_url" =~ ^git@github\.com:(.+)\.git$ ]]; then
    echo "https://github.com/${BASH_REMATCH[1]}"
  elif [[ "$remote_url" =~ ^https://github\.com/(.+)\.git$ ]]; then
    echo "https://github.com/${BASH_REMATCH[1]}"
  elif [[ "$remote_url" =~ ^https://github\.com/(.+)$ ]]; then
    echo "https://github.com/${BASH_REMATCH[1]}"
  fi
}

repo_url="$(resolve_github_repo_url)"

if [[ -z "$since_ref" ]]; then
  while IFS= read -r tag; do
    if [[ "$tag" != "$version" ]]; then
      since_ref="$tag"
      break
    fi
  done < <(git tag --sort=-creatordate)
fi

range="HEAD"
if [[ -n "$since_ref" ]]; then
  range="${since_ref}..HEAD"
fi

declare -a added=()
declare -a fixed=()
declare -a changed=()
declare -a docs=()
declare -a maintenance=()

append_commit() {
  local bucket="$1"
  local line="$2"
  case "$bucket" in
    added) added+=("$line") ;;
    fixed) fixed+=("$line") ;;
    changed) changed+=("$line") ;;
    docs) docs+=("$line") ;;
    maintenance) maintenance+=("$line") ;;
  esac
}

while IFS='|' read -r subject short_sha full_sha; do
  [[ -z "${subject// }" ]] && continue
  if [[ -n "$repo_url" ]]; then
    sha_ref="[\`${short_sha}\`](${repo_url}/commit/${full_sha})"
  else
    sha_ref="\`${short_sha}\`"
  fi
  line="- ${subject} (${sha_ref})"
  lower="$(printf '%s' "$subject" | tr '[:upper:]' '[:lower:]')"

  if [[ "$lower" =~ ^feat(\(.+\))?: ]] || [[ "$lower" =~ (add|support|introduce) ]]; then
    append_commit added "$line"
  elif [[ "$lower" =~ ^fix(\(.+\))?: ]] || [[ "$lower" =~ (bug|hotfix|regression|panic) ]]; then
    append_commit fixed "$line"
  elif [[ "$lower" =~ ^docs(\(.+\))?: ]] || [[ "$lower" =~ (readme|changelog|document) ]]; then
    append_commit docs "$line"
  elif [[ "$lower" =~ ^(chore|ci|build|test)(\(.+\))?: ]] || [[ "$lower" =~ (workflow|hook|deps|dependency) ]]; then
    append_commit maintenance "$line"
  else
    append_commit changed "$line"
  fi
done < <(git log --no-merges --pretty=format:'%s|%h|%H' "$range")

entry_file="$(mktemp)"
{
  echo "## [${version}] - $(date +%Y-%m-%d)"
  echo

  print_section() {
    local title="$1"
    shift
    local -n items="$1"
    [[ ${#items[@]} -eq 0 ]] && return 0
    echo "### ${title}"
    for item in "${items[@]}"; do
      echo "$item"
    done
    echo
  }

  print_section "Added" added
  print_section "Changed" changed
  print_section "Fixed" fixed
  print_section "Documentation" docs
  print_section "Maintenance" maintenance

  if [[ ${#added[@]} -eq 0 && ${#changed[@]} -eq 0 && ${#fixed[@]} -eq 0 && ${#docs[@]} -eq 0 && ${#maintenance[@]} -eq 0 ]]; then
    echo "### Changed"
    echo "- No notable changes."
    echo
  fi
} >"$entry_file"

if [[ "$dry_run" == "1" ]]; then
  cat "$entry_file"
  rm -f "$entry_file"
  exit 0
fi

changelog_path="$repo_root/CHANGELOG.md"
if [[ ! -f "$changelog_path" ]]; then
  cat >"$changelog_path" <<'EOF'
# Changelog

All notable changes to this project are documented in this file.

<!-- changelog-entries -->
EOF
fi

if ! grep -q '^<!-- changelog-entries -->$' "$changelog_path"; then
  printf '\n<!-- changelog-entries -->\n' >>"$changelog_path"
fi

if grep -q "^## \[${version//./\\.}\] - " "$changelog_path"; then
  echo "Version ${version} already exists in CHANGELOG.md" >&2
  rm -f "$entry_file"
  exit 1
fi

tmp_out="$(mktemp)"
awk -v entry_file="$entry_file" '
  {
    print
    if (!inserted && $0 == "<!-- changelog-entries -->") {
      print ""
      while ((getline line < entry_file) > 0) print line
      close(entry_file)
      inserted = 1
    }
  }
' "$changelog_path" >"$tmp_out"

mv "$tmp_out" "$changelog_path"
rm -f "$entry_file"
echo "Updated CHANGELOG.md for ${version} (range: ${range})"
