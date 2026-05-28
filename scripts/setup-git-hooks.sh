#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
hooks_dir="$repo_root/.githooks"

if [[ ! -d "$hooks_dir" ]]; then
  echo "Hooks directory not found: $hooks_dir" >&2
  exit 1
fi

chmod +x "$hooks_dir/pre-commit" "$hooks_dir/pre-push"
chmod +x \
  "$repo_root/scripts/quality-gate.sh" \
  "$repo_root/scripts/verify-tag-changelog.sh" \
  "$repo_root/.claude/hooks/check-before-commit.sh" \
  "$repo_root/.claude/hooks/check-before-push.sh" \
  "$repo_root/.codex/hooks/pre-commit.sh" \
  "$repo_root/.codex/hooks/pre-push.sh"
git -C "$repo_root" config core.hooksPath .githooks

echo "Git hooks enabled via core.hooksPath=.githooks"
