#!/usr/bin/env bash
# TaskCompleted hook: verify quality gates before Claude can declare "done".
# Checks for uncommitted changes, unpushed commits, and lint errors in changed files.
# Exit 0 = allow, Exit 2 = block (feedback sent to Claude).
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
ERRORS=""

# Check for uncommitted changes (Claude shouldn't finish with unstaged work)
UNSTAGED=$(git -C "$REPO_ROOT" diff --name-only 2>/dev/null || true)
STAGED=$(git -C "$REPO_ROOT" diff --cached --name-only 2>/dev/null || true)
if [[ -n "$UNSTAGED" || -n "$STAGED" ]]; then
  ERRORS="${ERRORS}Uncommitted changes detected. Commit or stash before finishing.\n"
fi

# Check if branch is ahead of remote (unpushed commits)
UNPUSHED=$(git -C "$REPO_ROOT" log "@{upstream}..HEAD" --oneline 2>/dev/null || true)
if [[ -n "$UNPUSHED" ]]; then
  ERRORS="${ERRORS}Unpushed commits detected. Push before finishing.\n"
fi

# Quick lint check on changed files vs main
CHANGED=$(git -C "$REPO_ROOT" diff --name-only "origin/main..HEAD" 2>/dev/null || true)
if [[ -z "$CHANGED" ]]; then
  exit 0
fi

HAS_TS=$(echo "$CHANGED" | grep -E '\.(ts|tsx)$' || true)
HAS_PY=$(echo "$CHANGED" | grep -E '\.py$' || true)

BUN="$(command -v bun || true)"
UV="$(command -v uv || true)"

if [[ -n "$HAS_TS" && -n "$BUN" ]]; then
  TS_FILES=$(echo "$HAS_TS" | tr '\n' ' ')
  if ! (cd "$REPO_ROOT" && $BUN biome check --no-errors-on-unmatched $TS_FILES) 2>&1 >/dev/null; then
    ERRORS="${ERRORS}Biome lint errors in changed TypeScript files.\n"
  fi
fi

if [[ -n "$HAS_PY" && -n "$UV" ]]; then
  PY_FILES=$(echo "$HAS_PY" | tr '\n' ' ')
  if ! (cd "$REPO_ROOT" && $UV run ruff check $PY_FILES) 2>&1 >/dev/null; then
    ERRORS="${ERRORS}Ruff lint errors in changed Python files.\n"
  fi
fi

if [[ -n "$ERRORS" ]]; then
  echo -e "Task completion blocked:\n$ERRORS" >&2
  echo "Fix the above before marking the task as done." >&2
  exit 2
fi

exit 0
