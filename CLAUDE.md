<!--
Tool-specific adapter for Claude Code. Portable instructions live in
`AGENTS.md` and `.agents/`; this file adds Claude-only surfaces.
-->

# CLAUDE.md

@AGENTS.md

Claude Code configuration for this repo.

## Agent skills

Issues and PRDs are tracked in GitHub Issues for this repo via the `gh` CLI.

## Claude-Specific Rule Loading

- Portable path-scoped rules are authored in `.agents/rules/*.md`.
- `.claude/rules/*.md` entries are symlinks to those portable sources so
  Claude's `paths:` frontmatter applies without duplicating rule bodies.
- Keep new durable coding guidance in `AGENTS.md`, nested `AGENTS.md`, or
  `.agents/rules/`; reserve this file for Claude-only behavior.

## Hooks

- **PreToolUse (Bash, Write/Edit/MultiEdit/NotebookEdit)**: `.claude/hooks/repo-guard.sh` — blocks tool calls touching paths outside the repo.
- **PreToolUse (Bash)**: `.claude/hooks/ci-check.sh` — runs scoped CI-equivalent checks before `git commit`/`git push`.
- **TaskCompleted**: `.claude/hooks/task-completed.sh` — checks for uncommitted changes, unpushed commits, and lint errors in changed files.
- **CwdChanged**: `.claude/hooks/cwd-changed.sh` — notes `.env` / `.envrc` files when changing directories.

## Agents

- `docs-maintainer` — `.claude/agents/docs-maintainer.md`
- `security-reviewer` — `.claude/agents/security-reviewer.md`
- `perf-reviewer` — `.claude/agents/perf-reviewer.md`
- `oracle` — `.claude/agents/oracle.md`
- `implementer` — `.claude/agents/implementer.md`
- `test-runner` — `.claude/agents/test-runner.md`

## Skills

All skills are portable: canonical sources live in `.agents/skills/`; Claude sees
them through `.claude/skills/` symlinks. See the skill table in `AGENTS.md`.
