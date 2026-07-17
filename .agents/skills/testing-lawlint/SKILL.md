---
name: testing-lawlint
description: Runtime-test the lawlint Rust rewrite end-to-end (desktop Tauri app, website WASM playground, download page). Use when verifying lawlint UI or engine changes.
---

# Testing lawlint (Rust core + CLI + WASM playground + Tauri desktop)

The CLI, the website playground (WASM), and the desktop app (Tauri) all call the **same**
`lawlint_core::lint`. So the fastest way to get hard pass/fail values is to run the native CLI
and assert the UIs match it.

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
- **Desktop app:** `bun run desktop:dev` (= tauri dev). Opens a native window titled "lawlint".
  Maximize before recording: `DISPLAY=:0 wmctrl -a lawlint && DISPLAY=:0 wmctrl -r lawlint -b add,maximized_vert,maximized_horz`.
  The desktop sample ("Load sample") text is hard-coded in `apps/desktop/src/main.ts`; running the CLI on
  that exact text gives the expected score/issues.

## Gotchas (may or may not still apply — verify)
- **Em-dash typing:** the computer-use `type` action cannot emit `—` (U+2014). Pasting/typing text with
  em-dashes into the playground silently drops them, which removes `no-em-dash`/`no-em-dash-overuse`
  diagnostics and changes the score. When comparing playground vs CLI, run the CLI on the *dash-stripped*
  text (`s.replace('\u2014','')`) to get the matching ground truth, or the counts won't line up.
- **Astro `define:vars` scripts are NOT type-stripped.** A `<script define:vars={...}>` is emitted inline
  verbatim, so any TypeScript-only syntax (`type` aliases, `querySelector<T>()` generics, `as` casts)
  becomes a browser SyntaxError and the whole script silently fails to run. This broke `/download` OS
  detection (everyone saw the macOS default). To check quickly:
  `curl -s localhost:4321/download | grep -o 'querySelector<\|type UserAgentData\|as Navigator'` should
  return nothing. `bun run build`/`typecheck` will NOT catch this — you must load the page or curl it.

## Quick checks
- `bun run build`, `bun run lint`, `bun run typecheck`
- `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`

## Environment / secrets
- The blueprint already installs Bun, Rust stable, rustfmt/clippy, `wasm32-unknown-unknown`, wasm-pack,
  Tauri Linux deps, and Tauri CLI. No extra setup needed to run the surfaces.
- Release publishing (not testable locally) needs repo secrets `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`,
  `R2_ACCOUNT_ID`, `R2_BUCKET`. Website deploy needs `CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID`.
