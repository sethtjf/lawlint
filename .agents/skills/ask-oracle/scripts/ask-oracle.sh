#!/usr/bin/env bash
# Ask a supervisor LLM (the "oracle") a single self-contained question.
# Usage:
#   ask-oracle.sh "question"            # one-shot question
#   <context> | ask-oracle.sh "question"  # attach context (diffs, logs) on stdin
# Env:
#   ANTHROPIC_API_KEY  required
#   ORACLE_MODEL       default: fable-5
#   ORACLE_MAX_TOKENS  default: 4096
#   ORACLE_BASE_URL    default: https://api.anthropic.com
set -euo pipefail

MODEL="${ORACLE_MODEL:-fable-5}"
MAX_TOKENS="${ORACLE_MAX_TOKENS:-4096}"
BASE_URL="${ORACLE_BASE_URL:-https://api.anthropic.com}"

if [[ $# -lt 1 || -z "$1" ]]; then
  echo "usage: $(basename "$0") \"question\" [< context]" >&2
  exit 2
fi
if [[ -z "${ANTHROPIC_API_KEY:-}" ]]; then
  echo "error: ANTHROPIC_API_KEY is not set" >&2
  exit 1
fi
command -v jq >/dev/null || { echo "error: jq is required" >&2; exit 1; }

QUESTION="$1"

CONTEXT=""
if [[ ! -t 0 ]]; then
  CONTEXT="$(head -c 51200)" # cap stdin context at 50KB
fi

SYSTEM="You are a senior engineering oracle supervising an autonomous coding \
agent. The agent escalates one hard question at a time; you have no other \
session context. Give a direct, decision-shaped answer: state your \
recommendation first, then the key reasoning, then risks or checks the agent \
should perform before acting. Be concise. If the question is unanswerable \
without information only the human user has, say so explicitly."

if [[ -n "$CONTEXT" ]]; then
  USER_CONTENT="$QUESTION"$'\n\n<context>\n'"$CONTEXT"$'\n</context>'
else
  USER_CONTENT="$QUESTION"
fi

BODY="$(jq -n \
  --arg model "$MODEL" \
  --argjson max_tokens "$MAX_TOKENS" \
  --arg system "$SYSTEM" \
  --arg content "$USER_CONTENT" \
  '{model: $model, max_tokens: $max_tokens, system: $system,
    messages: [{role: "user", content: $content}]}')"

RESPONSE="$(curl -sS --fail-with-body "$BASE_URL/v1/messages" \
  -H "x-api-key: $ANTHROPIC_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -H "content-type: application/json" \
  -d "$BODY")" || { echo "error: oracle request failed: $RESPONSE" >&2; exit 1; }

echo "$RESPONSE" | jq -r '.content[] | select(.type == "text") | .text'
