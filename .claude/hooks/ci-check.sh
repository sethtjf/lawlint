#!/usr/bin/env bash
# Pre-tool-use hook: runs CI-equivalent checks before git commit/push.
# Detects which packages have changed files and runs only those checks.
# Exit 0 = allow, Exit 2 = block (feedback sent to Claude).
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"

INPUT=$(cat)
COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

# Only intercept git commit and git push
IS_COMMIT=false
IS_PUSH=false
if echo "$COMMAND" | grep -qE '(^|\s|&&)git\s+((-C\s+\S+\s+)?commit|commit)'; then
  IS_COMMIT=true
elif echo "$COMMAND" | grep -qE '(^|\s|&&)git\s+((-C\s+\S+\s+)?push|push)'; then
  IS_PUSH=true
fi

if [[ "$IS_COMMIT" == false && "$IS_PUSH" == false ]]; then
  exit 0
fi

# Determine changed files
CHANGED_FILES=""
if [[ "$IS_COMMIT" == true ]]; then
  CHANGED_FILES=$(git -C "$REPO_ROOT" diff --cached --name-only 2>/dev/null || true)
  if [[ -z "$CHANGED_FILES" ]]; then
    # Nothing staged — might be using -a flag or HEREDOC commit
    CHANGED_FILES=$(git -C "$REPO_ROOT" diff --name-only 2>/dev/null || true)
  fi
elif [[ "$IS_PUSH" == true ]]; then
  CHANGED_FILES=$(git -C "$REPO_ROOT" diff --name-only "@{upstream}..HEAD" 2>/dev/null || true)
  if [[ -z "$CHANGED_FILES" ]]; then
    CHANGED_FILES=$(git -C "$REPO_ROOT" diff --name-only "origin/main..HEAD" 2>/dev/null || true)
  fi
fi

if [[ -z "$CHANGED_FILES" ]]; then
  exit 0
fi

# Detect which packages need checking
CHECK_BIOME=false
CHECK_COLLECTION_INDEXER_TSC=false
CHECK_WEB_TSC=false
CHECK_ATLAS=false

while IFS= read -r file; do
  case "$file" in
    apps/workflows/collection-indexer/*) CHECK_BIOME=true; CHECK_COLLECTION_INDEXER_TSC=true ;;
    apps/api-rs/*|packages/api-client/*)
      CHECK_BIOME=true ;;
    apps/web/*|packages/ui/*)
      CHECK_BIOME=true; CHECK_WEB_TSC=true ;;
    apps/marketing/*)
      CHECK_BIOME=true ;;
  esac
  case "$file" in
    packages/db/migrations/*.sql) CHECK_ATLAS=true ;;
  esac
done <<< "$CHANGED_FILES"

# Atlas: keep atlas.sum in sync whenever migration SQL files change
if [[ "$CHECK_ATLAS" == true && "$IS_COMMIT" == true ]]; then
  atlas migrate hash --env local --dir "file://$REPO_ROOT/packages/db/migrations" 2>/dev/null || true
  git -C "$REPO_ROOT" add packages/db/migrations/atlas.sum
fi

ERRORS=""

# TypeScript: biome (staged files) + tsc (per-package)
BUN="$(command -v bun)"
if [[ "$CHECK_BIOME" == true ]]; then
  if ! (cd "$REPO_ROOT" && $BUN biome check --staged --no-errors-on-unmatched) 2>&1; then
    ERRORS="${ERRORS}biome check failed\n"
  fi
fi
# tsc is project-wide; filter output to only fail on errors in changed files
run_tsc_scoped() {
  local label="$1"
  local tsconfig="$2"
  local tsc_output
  tsc_output=$( (cd "$REPO_ROOT" && $BUN tsc --noEmit -p "$tsconfig") 2>&1 ) && return 0
  local changed_errors=""
  while IFS= read -r file; do
    local matches
    matches=$(echo "$tsc_output" | grep "^$file" || true)
    if [[ -n "$matches" ]]; then
      changed_errors="${changed_errors}${matches}\n"
    fi
  done <<< "$CHANGED_FILES"
  if [[ -n "$changed_errors" ]]; then
    echo -e "$changed_errors" >&2
    ERRORS="${ERRORS}tsc failed for $label (errors in changed files)\n"
  fi
}

if [[ "$CHECK_COLLECTION_INDEXER_TSC" == true ]]; then
  run_tsc_scoped "collection-indexer" "$REPO_ROOT/apps/workflows/collection-indexer/tsconfig.json"
fi
if [[ "$CHECK_WEB_TSC" == true ]]; then
  run_tsc_scoped "web" "$REPO_ROOT/apps/web/tsconfig.json"
fi

if [[ -n "$ERRORS" ]]; then
  echo -e "CI checks failed before ${IS_COMMIT:+commit}${IS_PUSH:+push}:\n$ERRORS" >&2
  echo "Fix the errors above before retrying." >&2
  exit 2
fi

exit 0
