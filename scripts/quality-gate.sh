#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "Usage: $0 <pre-commit|pre-push> [--claude]" >&2
  exit 2
fi

phase="$1"
mode="${2:-plain}"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [[ "${NANOBOT_SKIP_HOOKS:-0}" == "1" ]]; then
  exit 0
fi

case "$phase" in
  pre-commit) recipe="hook-commit" ;;
  pre-push)
    recipe="hook-push"
    # Git pre-push hooks pass ref updates through stdin:
    # <local-ref> <local-sha> <remote-ref> <remote-sha>
    # Enforce changelog entry for pushed tags before running heavier checks.
    if [[ ! -t 0 ]]; then
      refspec_file="$(mktemp)"
      cat >"$refspec_file"
      trap 'rm -f "$refspec_file"' EXIT
      bash "$repo_root/scripts/verify-tag-changelog.sh" "$refspec_file"
    fi
    ;;
  *)
    echo "Unknown phase: $phase" >&2
    exit 2
    ;;
esac

if just "$recipe"; then
  exit 0
fi

if [[ "$mode" == "--claude" ]]; then
  reason="Blocked ${phase}: quality gate '${recipe}' failed."
  printf '%s\n' \
    "{\"hookSpecificOutput\":{\"hookEventName\":\"PreToolUse\",\"permissionDecision\":\"deny\",\"permissionDecisionReason\":\"$reason\"}}"
  exit 0
fi

exit 1
