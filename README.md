# lawlint

[![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

`lawlint` flags patterns that can make legal and general prose sound machine-generated,
and offers practical suggestions for more human, direct writing.

## Quickstart

**1. Install** (Windows PowerShell: `irm https://lawlint.com/install.ps1 | iex`):

```sh
curl -fsSL https://lawlint.com/install.sh | sh
```

**2. Lint a document** — this needs no configuration and runs entirely offline:

```sh
lawlint contract.docx      # or .txt / .md / "-" for stdin
```

You get a list of findings and a human-likeness score. `--fix` applies the
machine-applicable fixes (for `.docx`, as native Word tracked changes).

**3. Set up AI features** (optional — the tier-3 judge and `lawlint learn`):

```sh
lawlint init
```

The walkthrough asks which AI model lawlint should use. Hosted providers
(Anthropic — recommended, OpenAI, Azure Foundry) need an API key, which is
stored in a user-level credential file — never in your project. AI features
are off until a model is configured; they error with guidance rather than
downloading anything silently.

The [download page](https://lawlint.com/download) also has the unsigned desktop
app for macOS and Windows, plus direct CLI archives for every supported
platform. The installers place the CLI in a user-local bin directory and do
not send documents anywhere. To build from source: `cargo build --release -p
lawlint-cli`.

## Setup & configuration

`lawlint init` walks through AI-model, judge, Markdown, and custom-rule
choices and writes `.lawlint/config.json` (plus an optional starter rules
package in `.lawlint/rules/`). The CLI discovers `.lawlint/config.json` — or
the legacy `lawlint.config.json` — from the current directory upward.

```jsonc
// .lawlint/config.json (all fields optional, camelCase)
{
  "ai": {
    "model": "anthropic:claude-haiku-4-5-20251001", // default for all AI features
    "features": { "judge": "...", "learn": "..." }, // optional per-feature overrides
    "localAcknowledged": false                       // set by init's local-model consent step
  },
  "judge": { "enabled": false },  // run the tier-3 judge on every lint
  "markdown": false,              // treat stdin as Markdown
  "ruleDirs": [".lawlint/rules"]  // extra rule packages, merged over built-ins
}
```

**Hosted vs local models.** Hosted providers are the recommended path: better
quality, no downloads. API keys go to a user-level credential file
(`~/.config/lawlint/credentials`, `0600`), or set `ANTHROPIC_API_KEY` /
`OPENAI_API_KEY` / `AZURE_FOUNDRY_API_KEY` in the environment (env wins).
Local models (Qwen 2.5, Gemma — via the embedded mistral.rs runtime) remain
available as an explicit advanced choice: they involve multi-GB downloads,
slower inference, and measurably lower quality (see `docs/eval-corpus.md`),
and init asks you to acknowledge that before enabling one. Non-interactive
setup: `lawlint init --yes` (hosted default), or `--ai qwen
--acknowledge-local` for a local model.

## Everyday commands

```sh
lawlint brief.docx                 # lint; prints findings + human-likeness score
lawlint --fix brief.docx           # apply fixes (tracked changes + comments in Word)
lawlint --diff draft.md            # preview what --fix would change
lawlint --judge draft.md           # add tier-3 AI-judged findings (needs init)
lawlint --format prompt draft.md   # emit an AI revision brief for your assistant
lawlint learn ~/my-writing/        # mine a personal rule package from your prose
lawlint rules --json               # built-in rule metadata
```

The human-likeness score (0–100) aggregates only `detection`-intent rules —
the ones corpus-validated to distinguish AI from human legal prose. `style`
rules (Oxford comma, semicolons, sentence length, Orwell rules, …) still
report findings and participate in `--fix`, but never move the score.
`lawlint learn` generates rules from *your* writing: a statistical pass over
the full corpus, an AI mining pass over a small sample, and a self-consistency
gate so no generated rule flags your own prose.

### File formats

The CLI and desktop app lint plain text, Markdown (`.md`), and Word documents
(`.docx`). For `.docx`, text is projected out of the document for linting; with
`--fix`, machine-applicable fixes are written back as native Word **tracked
changes** with a review **comment** per fix, so every change can be accepted or
rejected in Word. All other parts of the document are preserved byte-for-byte.
Fixes whose span crosses multiple runs are reported and skipped rather than
applied (not yet supported).

## Rust SDK

```rust
use lawlint_core::{lint, LintOptions};

let result = lint(
    "It is important to note that we delve into the landscape.",
    &LintOptions::default(),
);
```

`lawlint-core` is a pure Rust SDK and does not perform file or stdin I/O. The
website's [playground](https://lawlint.com/playground) uses the same engine
compiled to WebAssembly.

## Monorepo layout

- `crates/lawlint-core` — pure-Rust linting SDK.
- `crates/lawlint-cli` — native CLI.
- `crates/lawlint-docx` — read `.docx` into the text model and write fixes back as tracked changes + comments.
- `crates/lawlint-wasm` — browser binding used by the playground.
- `apps/website` — Astro documentation website with generated rule reference pages.
- `.github/workflows/ci.yml` — Rust checks plus the Bun/Astro website build.
- `.github/workflows/release.yml` — tagged CLI/desktop builds, R2 uploads, and release notes.

To work on the documentation website locally: `bun install && bun run --cwd
apps/website dev`.

## Maintainer releases

Pushing a `v*` tag runs the release workflow. It publishes versioned and
`latest/` assets to the R2 bucket behind `https://assets.lawlint.com/downloads`, then
creates a GitHub Release whose notes point to those canonical download URLs.
The workflow requires these repository secrets:

- `R2_ACCESS_KEY_ID`
- `R2_SECRET_ACCESS_KEY`
- `R2_ACCOUNT_ID`
- `R2_BUCKET`

`GITHUB_TOKEN` is supplied automatically by GitHub Actions. The public download
base is defined in `apps/website/src/config/downloads.ts` and mirrored in the
release workflow and install scripts so the distribution domain can be changed
in one reviewable place.

## License

MIT. See [LICENSE](LICENSE).
