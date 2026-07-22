# Coding Agent Instructions

Universal instructions for AI coding agents working in the `lawlint` repo. This
file is the portable source of truth for Claude Code, Devin, and Codex;
tool-specific adapters live in `CLAUDE.md` and `.claude/`.

---

## Rule Portability Model

Portable coding-agent guidance lives in this root `AGENTS.md` and in the
`.agents/rules/*.md` path-scoped rules. `.agents/skills/` holds reusable
workflows.

- Keep repo-wide guidance in this file.
- Put directory-specific guidance in `.agents/rules/<rule>.md` with a `paths:`
  frontmatter block.
- Put reusable workflows in `.agents/skills/<skill>/SKILL.md`.
- Put tool-specific behavior in `CLAUDE.md` / `.claude/` only.

The `.claude/rules/`, `.claude/skills/`, and `.devin/skills/` directories are
symlinks to `.agents/rules/` and `.agents/skills/` so every agent reads the same
source files.

To re-sync the shared `.agents/`, `.claude/`, and `.devin/` trees from the
`Litvue/agents` source of truth, run:

```bash
bun run sync-agent-config
```

---

## Project Overview

`lawlint` flags patterns that can make legal and general prose sound
machine-generated, and offers practical suggestions for more human, direct
writing.

- **Core engine:** Rust (`crates/lawlint-core`)
- **CLI:** Rust (`crates/lawlint-cli`)
- **Website:** Astro + WASM playground (`apps/website`)
- **Desktop:** Tauri v2 (`apps/desktop`)

See `README.md` for setup, everyday commands, and configuration.

---

## Core Commands

Run from the repo root.

```bash
# Rust
cargo test --workspace
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
cargo build --release -p lawlint-cli

# JavaScript / website (Bun workspaces)
bun install
bun run lint
bun run typecheck
bun run build
bun run dev

# Generate the website rules.json page
./target/release/lawlint rules --json
```

Releases are handled by `.github/workflows/release.yml`.

---

## Verification Defaults

After code changes, verify in this order:

1. **Targeted checks first** — run the narrowest command that exercises the
   changed code (e.g., `cargo test -p lawlint-core` or `bunx biome check
   apps/website`).
2. **Repo-level lint + typecheck** — `cargo clippy`, `cargo fmt`, and `bun run
   lint` / `bun run typecheck` as relevant.
3. **Build** — `cargo build` or `bun run build` when changes affect build paths,
   generated artifacts, or config.
4. **E2E / manual** — run the website or CLI against fixtures only when behavior
   changed.

If a check is skipped, state why.

No tech debt may be left behind. If you took shortcuts, introduced temporary
workarounds, duplicated logic, or deferred cleanup, go back and do it right
before declaring completion.

---

## Code Conventions

### Rust

- `cargo fmt` is the source of truth for formatting.
- `cargo clippy --all-targets -- -D warnings` must pass.
- Keep `unsafe` minimal and well-documented.

### TypeScript / website

- Biome config in `biome.json`.
- `bun run lint` and `bun run typecheck` must pass.
- The website runs the Rust core through `wasm-pack`.

### Commit & PR Conventions

PR titles **must** follow [Conventional Commits](https://conventionalcommits.org):

```
type(scope): description
```

| Type | Use For | Version Bump |
|------|---------|--------------|
| `feat` | New feature | minor |
| `fix` | Bug fix | patch |
| `perf` | Performance improvement | patch |
| `refactor` | Code restructure (no behavior change) | patch |
| `style` | Formatting, whitespace | patch |
| `test` | Adding/updating tests | patch |
| `build` | Build system, dependencies | patch |
| `chore` | Maintenance, tooling | patch |
| `revert` | Revert previous commit | patch |
| `docs` | Documentation only | skipped |
| `ci` | CI/CD configuration | skipped |

- Type and description must be lowercase.
- Use imperative mood: "add" not "added".
- Keep the subject line under 72 characters with no trailing period.
- Breaking changes: add `!` after the type, e.g. `feat(cli)!: ...`.

### Comment Policy

- No comments that repeat what code does.
- No commented-out code (delete it).
- Code should be self-documenting; if a comment is needed to explain WHAT the
  code does, consider refactoring.

### Documentation

- Most docs live in `README.md` files colocated with the code they describe.
- Public rule docs and website content live in `apps/website/`.
- Do not create new top-level `docs/` subdirectories without a decision.

---

## Unit Testing

- Rust: co-located `#[cfg(test)]` modules and `crates/<crate>/tests/*.rs` files.
- Website: keep Playwright/Manual checks in `.github/workflows/` and run them
  only when UI behavior changed.
- Never check in real credentials or binary artifacts.

---

## Portable Workflow Skills

Canonical portable skills live under `.agents/skills/` and are exposed to
Claude Code through `.claude/skills/` and to Devin through `.devin/skills/`.

| Skill | Use |
|-------|-----|
| `roundup` | Reconcile open PRs, issues, and sessions into one plan. |
| `handoff` | Compact the conversation for another agent. |
| `to-issues` | Break a plan into tracer-bullet issues. |
| `grill` | Stress-test a design or plan. |
| `prototype` | Throwaway prototype for ambiguous features. |
| `diagnose` | Disciplined debugging loop. |
| `ask-oracle` | Escalate a hard question to a stronger model. |
| `deliver` | Execute one scoped item to completion. |
| `testing-lawlint` | Repo-specific end-to-end testing workflow. |

---

## Agent-Specific Configuration

| Agent | Config Location | Purpose |
|-------|-----------------|---------|
| Claude Code | `CLAUDE.md` + `.claude/` | Tool-only commands, hooks, subagents, skills. |
| Devin | `AGENTS.md` + `.agents/skills/` | Reads portable instructions and skills. |
| OpenAI Codex | `AGENTS.md` + `.agents/skills/` | Reads portable instructions and skills. |

See `CLAUDE.md` for Claude Code-only surfaces.
