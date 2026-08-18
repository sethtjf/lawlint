---
name: testing-lawlint
description: Runtime-test the lawlint Rust rewrite end-to-end (CLI, website WASM playground, download page). Use when verifying lawlint UI or engine changes.
---

# Testing lawlint (Rust core + CLI + WASM playground)

The CLI and website playground (WASM) call the **same**
`lawlint_core::lint`. So the fastest way to get hard pass/fail values is to run the native CLI
and assert the UIs match it.

The former Tauri desktop app under `apps/desktop` is an archived prototype, not an active
workspace or release surface. Do not include it in runtime verification unless deliberately
working on a future reintroduction.

## Derive ground truth from the CLI
```bash
cargo run -q -p lawlint-cli -- --format json <file> \
  | python3 -c "import sys,json;d=json.load(sys.stdin);print(d['stats'])"
```
Compare `score`, `wordCount`, `sentenceCount`, and diagnostic count / rule IDs against what the UI shows.

## Run the surfaces locally
- **Website (playground + download page):** `bun run dev` from repo root → http://localhost:4321
  (`/playground`, `/download`). This runs `wasm-pack build` + generates `rules.json` before astro dev,
  so first start takes ~15-30s.

## Gotchas (may or may not still apply — verify)
- **Em-dash typing:** the computer-use `type` action cannot emit `—` (U+2014). Pasting/typing text with
  em-dashes into the playground silently drops them, which removes `no-em-dash-overuse`
  diagnostics and changes the score. When comparing playground vs CLI, run the CLI on the *dash-stripped*
  text (`s.replace('\u2014','')`) to get the matching ground truth, or the counts won't line up.
- **Astro `define:vars` scripts are NOT type-stripped.** A `<script define:vars={...}>` is emitted inline
  verbatim, so any TypeScript-only syntax (`type` aliases, `querySelector<T>()` generics, `as` casts)
  becomes a browser SyntaxError and the whole script silently fails to run. This broke `/download` OS
  detection (everyone saw the macOS default). To check quickly:
  `curl -s localhost:4321/download | grep -o 'querySelector<\|type UserAgentData\|as Navigator'` should
  return nothing. `bun run build`/`typecheck` will NOT catch this — you must load the page or curl it.

## Testing custom rule packages (Markdown rule files)
Rule packages load via `--rule-dir <dir>`: `<dir>/style.yaml` (`name`, `version`) plus `rules/*.md`.
Every rule (hard and soft) is a Claude Code-style Markdown file: YAML frontmatter carries all
structured fields (`id` and `engine` required); the body is the rubric for inferential (soft) rules
(with `## Flag examples`/`## Pass examples` sections) or optional explanation prose for hard rules,
exposed as `explanation` in `rules --json` and rendered on the website rule page.
- `lawlint rules --json --rule-dir <dir>` proves loading and shows the merged metadata (built-ins + package).
- `lawlint rules test <dir> --offline` validates flag/pass examples without an AI model (inferential
  examples are skipped offline — expected, not a failure).
- Validation errors are a product feature: assert exact messages (they include file path, field, value),
  e.g. "needs at least 3 flag_examples" or severity "too severe for an inferential rule".
- Frontmatter rejects unknown fields; `.yaml` files under `rules/` are ignored — a good negative
  test is dropping an old-format YAML rule in and asserting the rule count is unchanged.
- A live `--judge` run needs a configured AI model (`lawlint init`, hosted API key); without one, treat
  judge inference as untested rather than simulating it.
- The website /rules pages are generated from `lawlint rules --json`, so per-rule pages (including
  soft-rule "kind"/tier rows) can be cross-checked against the CLI output.

## Quick checks
- `bun run build`, `bun run lint`, `bun run typecheck`
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`,
  `cargo test --workspace --locked`

## Environment / secrets
- The blueprint already installs Bun, Rust stable, rustfmt/clippy, `wasm32-unknown-unknown`, and
  wasm-pack. No extra setup is needed for the active CLI and website surfaces. Tauri dependencies
  are intentionally not installed because the desktop prototype is archived.
- Release publishing (not testable locally) needs repo secrets `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`,
  `R2_ACCOUNT_ID`, `R2_BUCKET`. Website deploy needs `CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID`.
