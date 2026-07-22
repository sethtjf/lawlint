# Commands and Verification

## Verification Order

After code changes, verify in this order:

1. Run targeted checks first (Nx project target when available).
2. Run repo-level `just lint` and `just typecheck` for substantial changes.
3. Run `just build` when changes affect build paths, routing, generated clients, or config.
4. Run `bun run test:e2e` only when web/api behavior changed and staging validation is needed.

If you skip a check, state why.

## No Tech Debt Acceptance Criterion

Do not leave any tech debt behind. If you took shortcuts, introduced temporary
workarounds, duplicated logic, bypassed intended architecture, or deferred
cleanup during the task, go back and do it right before declaring completion.
This is a hard acceptance criterion for all implementation work.

## Targeted Command Patterns

Run from repo root.

```bash
# Target a single workspace/package
bun scripts/nx-run-target.ts lint
nx run web:typecheck
nx run web:build

nx run @litvue/ui:typecheck

# Rust API-focused changes
just build-api-rs
cargo test -p api-rs
```

## Notes

- Prefer fastest command that gives confidence first; then broaden if needed.
- Do not run E2E by default for pure refactors/docs/style-only changes.
