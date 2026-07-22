#!/usr/bin/env bash
# sync-agent-config.sh
#
# Pulls the shared coding-agent configuration (.agents, .claude, .devin) from
# the source of truth and merges it into this repo.
#
# Repo-local files (AGENTS.md and CLAUDE.md) are NOT overwritten; edit them
# directly for project-specific instructions.
#
# Usage:
#   ./scripts/sync-agent-config.sh
#
# Use a local checkout instead of cloning:
#   AGENTS_CONFIG_DIR=../agents ./scripts/sync-agent-config.sh
#
# Use a different remote:
#   AGENTS_CONFIG_REPO=https://github.com/owner/agent-config.git ./scripts/sync-agent-config.sh

set -euo pipefail

REPO_URL="${AGENTS_CONFIG_REPO:-https://github.com/Litvue/agents.git}"
LOCAL_DIR="${AGENTS_CONFIG_DIR:-}"

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

if [[ -n "$LOCAL_DIR" ]]; then
	if [[ ! -d "$LOCAL_DIR/.agents" ]]; then
		echo "AGENTS_CONFIG_DIR must point to a checkout containing .agents, .claude, and .devin" >&2
		exit 1
	fi
	SRC="$LOCAL_DIR"
else
	git clone --depth 1 "$REPO_URL" "$TMPDIR/agents"
	SRC="$TMPDIR/agents"
fi

for dir in .agents .claude .devin; do
	if [[ -d "$SRC/$dir" ]]; then
		mkdir -p "./$dir"
		cp -r "$SRC/$dir/." "./$dir/"
		echo "synced $dir"
	fi
done

echo "Agent config synced from $SRC."
echo "Review the diff, then commit. AGENTS.md and CLAUDE.md were not touched."
