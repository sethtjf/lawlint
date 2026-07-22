#!/usr/bin/env bash
# CwdChanged hook: auto-load environment context when Claude changes directories.
# Detects .dev.vars and .envrc files in the new directory.
# Exit 0 = allow (always allow directory changes).
set -euo pipefail

INPUT=$(cat)
NEW_DIR=$(echo "$INPUT" | jq -r '.cwd // empty')

if [[ -z "$NEW_DIR" ]]; then
  exit 0
fi

# If entering an app with .dev.vars, remind about env context
if [[ -f "$NEW_DIR/.dev.vars" ]]; then
  echo "Note: $(basename "$NEW_DIR") has .dev.vars — run 'source .dev.vars' if env vars needed"
fi

# If entering a directory with a .envrc (direnv), note it
if [[ -f "$NEW_DIR/.envrc" ]]; then
  echo "Note: $(basename "$NEW_DIR") has .envrc — direnv will auto-load environment"
fi

exit 0
