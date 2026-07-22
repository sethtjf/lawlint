#!/usr/bin/env bash
# Pre-tool-use hook: hard-blocks any tool call touching paths outside the repo.
# Covers Bash commands and file tools (Write/Edit/MultiEdit/NotebookEdit).
# Exit 0 = allow, Exit 2 = block (feedback sent to Claude).
# Exit 2 blocks even in bypass-permissions ("yolo") mode; permissions.deny
# rules in .claude/settings.json act as a backup layer.
set -euo pipefail

REPO_ROOT="${CLAUDE_PROJECT_DIR:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}"
REPO_ROOT="$(realpath -m "$REPO_ROOT")"

INPUT=$(cat)
TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // empty')
HOOK_CWD=$(echo "$INPUT" | jq -r '.cwd // empty')
[[ -z "$HOOK_CWD" ]] && HOOK_CWD="$REPO_ROOT"

# Paths outside the repo that legitimate dev commands need (colon-separated).
# Extend per-machine via REPO_GUARD_ALLOW without editing this script.
DEFAULT_ALLOW="/dev:/proc:/sys:/tmp:/var/tmp:/usr:/bin:/sbin:/lib:/lib64:/opt:/etc:/run"
DEFAULT_ALLOW="$DEFAULT_ALLOW:$HOME/.cache:$HOME/.cargo:$HOME/.rustup:$HOME/.bun:$HOME/.local:$HOME/.npm:$HOME/.config:$HOME/.gitconfig"
ALLOW_PREFIXES="$DEFAULT_ALLOW${REPO_GUARD_ALLOW:+:$REPO_GUARD_ALLOW}"

is_allowed() {
  local resolved="$1"
  if [[ "$resolved" == "$REPO_ROOT" || "$resolved" == "$REPO_ROOT"/* ]]; then
    return 0
  fi
  local IFS=':'
  local prefix
  for prefix in $ALLOW_PREFIXES; do
    [[ -z "$prefix" ]] && continue
    if [[ "$resolved" == "$prefix" || "$resolved" == "$prefix"/* ]]; then
      return 0
    fi
  done
  return 1
}

block() {
  echo "repo-guard: blocked $TOOL_NAME touching path outside the repo: $1" >&2
  echo "Repo root is $REPO_ROOT. Work inside the repo, or extend REPO_GUARD_ALLOW if this path is genuinely needed." >&2
  exit 2
}

check_path() {
  local raw="$1"
  [[ -z "$raw" || "$raw" == "null" ]] && return 0
  local expanded="$raw"
  [[ "$expanded" == "~" ]] && expanded="$HOME"
  [[ "$expanded" == "~/"* ]] && expanded="$HOME/${expanded#\~/}"
  local resolved
  resolved="$(cd "$HOOK_CWD" 2>/dev/null && realpath -m -- "$expanded" 2>/dev/null || realpath -m -- "$expanded" 2>/dev/null || true)"
  [[ -z "$resolved" ]] && return 0
  is_allowed "$resolved" || block "$raw"
}

case "$TOOL_NAME" in
  Write|Edit|MultiEdit|NotebookEdit)
    FILE_PATH=$(echo "$INPUT" | jq -r '.tool_input.file_path // .tool_input.notebook_path // empty')
    check_path "$FILE_PATH"
    exit 0
    ;;
  Bash) ;;
  *) exit 0 ;;
esac

COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command // empty')
[[ -z "$COMMAND" ]] && exit 0

# Extract path-like tokens: absolute paths, tilde paths, and tokens with "..".
# Quotes are stripped; tokens containing shell expansions or URLs are skipped
# (they cannot be resolved statically and resolve relative to cwd otherwise).
while IFS= read -r token; do
  [[ -z "$token" ]] && continue
  token="${token%\"}" ; token="${token#\"}"
  token="${token%\'}" ; token="${token#\'}"
  [[ "$token" == *"://"* || "$token" == *'$'* || "$token" == *'`'* ]] && continue
  case "$token" in
    /*|"~"|"~/"*|*..*) check_path "$token" ;;
  esac
done < <(echo "$COMMAND" | tr ';|&<>()' ' ' | tr -s ' \t' '\n')

exit 0
