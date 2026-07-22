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

You get a summary of which rules ran, what they found, and a human-likeness
score; `--list` prints the findings themselves. `--fix` applies the fixes (for
`.docx`, as native Word tracked changes).

**3. Set up AI features** (optional — the soft-rule AI judge and `lawlint learn`):

```sh
lawlint init
```

The walkthrough asks which AI model lawlint should use. Hosted providers
(Anthropic — recommended, OpenAI, Azure Foundry) need an API key, which is
stored in a user-level credential file — never in your project.

Once a model and its key are configured, **the soft rules run on every lint**,
including from directories that contain no project config. Nothing is
downloaded silently: a `local:` model still waits for the explicit
acknowledgment `init` asks for, and with no model configured the soft rules are
skipped and the summary says so. `--no-ai` turns them off for a run.

The [download page](https://lawlint.com/download) also has the unsigned desktop
app for macOS and Windows, plus direct CLI archives for every supported
platform. The installers place the CLI in a user-local bin directory and do
not send documents anywhere. To build from source: `cargo build --release -p
lawlint-cli`.

## Setup & configuration

`lawlint init` walks through AI-model, judge, Markdown, and custom-rule
choices and writes `.lawlint/config.json` (plus an optional starter rules
package in `.lawlint/rules/`). The CLI discovers `.lawlint/config.json` — or
the legacy `lawlint.config.json` — from the current directory upward, then
falls back to a user-level `~/.lawlint/config.json` (`$LAWLINT_HOME`
honoured), beside the credential store — the same `.lawlint` name at both
scopes. That fallback is what makes lawlint work on documents that live
outside any project — a matter folder, a downloaded attachment. A project
config **layers over** the user-level one field by field, so a repo that pins
only `ruleDirs` still inherits your AI model rather than silently disabling the
soft rules.

Releases before 0.8 kept these under `~/.config/lawlint/`. That path is still
read, and the next `lawlint init` moves both files to `~/.lawlint/` and tells
you it did.

```jsonc
// .lawlint/config.json (all fields optional, camelCase)
{
  "ai": {
    "model": "anthropic:claude-haiku-4-5-20251001", // default for all AI features
    "features": { "judge": "...", "learn": "..." }, // optional per-feature overrides
    "localAcknowledged": false                       // set by init's local-model consent step
  },
  "judge": {
    "enabled": true,              // force the soft rules on/off; omit to let
                                  // them run whenever credentials allow
    "floor": 0.6,                 // minimum confidence for a judge finding
    "maxTokens": 16384,           // per-request generation budget; raise for
                                  // reasoning models (see below)
    "concurrency": 4,             // requests in flight at once (hosted only)
    "contextChars": 24000,        // document text per request
    "perRule": true               // one request per rule, not per section
  },
  "markdown": false,              // treat stdin as Markdown
  "ruleDirs": [".lawlint/rules"]  // extra rule packages, merged over built-ins
}
```

**Hosted vs local models.** Hosted providers are the recommended path: better
quality, no downloads. API keys go to a user-level credential file
(`~/.lawlint/credentials`, `0600`), or set `ANTHROPIC_API_KEY` /
`OPENAI_API_KEY` / `AZURE_FOUNDRY_API_KEY` in the environment (env wins).
Local models (Qwen 2.5, Gemma — via the embedded mistral.rs runtime) remain
available as an explicit advanced choice: they involve multi-GB downloads,
slower inference, and measurably lower quality (see `docs/eval-corpus.md`),
and init asks you to acknowledge that before enabling one. Non-interactive
setup: `lawlint init --yes` (hosted default), or `--ai qwen
--acknowledge-local` for a local model.

**How soft rules are batched.** Every field above has a backend-derived
default, so the defaults track the model rather than a constant. Hosted
backends get `contextChars: 24000` and `perRule: true` — each rule is judged
in its own request seeing the whole document (or a few large sections), and
requests run `concurrency` at a time. `local:` backends keep small sections
with every rubric bundled into one request and stay sequential, because a
1.5B model degrades on long context and a second in-process copy would mean a
second multi-GB model load.

Per-rule requests each carry the document, which sounds expensive and isn't:
the prompt puts the document *before* the rubric, so every rule request over
one document shares a byte-identical prefix that providers bill at a discount.
Measured on Azure Foundry with four rules over an 852-word memo, 72% of total
input tokens came back cached — the marginal cost of another rule is its
rubric, not another copy of the document.

The tradeoff to know: `contextChars` is also the cache granule. Bigger units
mean fewer requests and more cross-section context, but an edit anywhere in a
unit re-runs that whole unit. Lower it if you lint large documents on every
keystroke.

**Reasoning models.** `judge.maxTokens` caps what the model may generate per
request. On OpenAI-compatible routes that budget covers hidden reasoning
tokens as well as the findings array, so a thinking model can spend the whole
cap on reasoning and return nothing — every section then fails and lawlint
warns that the judge failed on N of N sections. If you see that, raise
`maxTokens`. Only tokens actually generated are billed, so headroom is cheap.

## Everyday commands

```sh
lawlint brief.docx                 # lint; prints a coverage summary + score
lawlint --list brief.docx          # …plus every finding, in document order
lawlint --coverage brief.docx      # which rules did not run, and why
lawlint --fix brief.docx           # apply fixes (tracked changes + comments in Word)
lawlint --diff draft.md            # preview what --fix would change
lawlint --no-ai brief.docx         # hard rules only, even when AI is configured
lawlint --format prompt draft.md   # emit an AI revision brief for your assistant
lawlint learn ~/my-writing/        # mine a personal rule package from your prose
lawlint rules --json               # built-in rule metadata
```

A bare lint prints what ran rather than a wall of findings:

```
  brief.docx   1,980 words · 100 sentences

  Static rules   15 run        14 findings
  Statistical    11 run         5 findings
  AI rules        2 run        16 findings

  Human-likeness  82/100

  35 findings · --list to see them · --fix to apply 16
```

When the soft rules cannot run, the summary says so and the score declares its
basis — a `97/100 (static rules only)` is never mistaken for the `82/100` the
same document scores with every tier running.

The human-likeness score (0–100) aggregates only `detection`-intent rules —
the ones corpus-validated to distinguish AI from human legal prose. `style`
rules (Oxford comma, semicolons, sentence length, Orwell rules, …) still
report findings and participate in `--fix`, but never move the score.
`lawlint learn` generates rules from *your* writing: a statistical pass over
the full corpus, an AI mining pass over a small sample, and a self-consistency
gate so no generated rule flags your own prose.

### Hard rules and soft rules

lawlint has two user-facing rule kinds. **Hard rules** are deterministic and
run offline: phrase and leading engines are tier `static`, while density and
statistical engines are tier `statistical`. **Soft rules** are inferential
rules evaluated by the AI judge (tier `inferential`), which runs whenever a
model and its credentials are configured. The serialized
`Tier::{Static, Statistical, Inferential}` values and rule-package fields stay
unchanged; hard/soft is the explanatory terminology used in the docs.

### Markdown rule files

Every rule is a Claude Code-style Markdown file. YAML frontmatter carries the
structured fields and must declare an explicit stable `id`. Hard rules (phrase,
leading, density, and statistical engines) may use the body for explanatory
prose. Soft rules (inferential/AI-judge rules) use the body as their rubric:

```markdown
---
id: empty-hedge
engine: inferential
severity: warning
---
Flag a sentence when it hedges a claim without saying what is uncertain or why.
```

Soft rules need at least three flag examples and three pass examples, either as
frontmatter arrays or in `## Flag examples` and `## Pass examples` sections.
The package manifest remains `style.yaml`; only `.md` files under `rules/` are
discovered as rules.

`description`, `severity`, `granularity`, `scope`, `intent`, `docs`, `message`,
and `rationale` are optional; granularity defaults to `sentence`. The
`flag_examples` and `pass_examples` arrays may instead be supplied in
frontmatter. Each list needs at least three examples, and soft-rule severity
is limited to `warning` or `suggestion`. The body outside the two example
sections is the rubric. YAML fields take precedence over frontmatter; do not
set both `rubric` and `skill`.

### File formats

The CLI and desktop app lint plain text, Markdown (`.md`), and Word documents
(`.docx`). For `.docx`, text is projected out of the document for linting; with
`--fix`, fixes are written back as native Word **tracked changes** with a
review **comment** per fix, so every change can be accepted or rejected in
Word. All other parts of the document are preserved byte-for-byte. Fixes whose
span crosses multiple runs, or that overlap a fix already applied, are reported
and skipped rather than applied.

Because a tracked change *is* a proposal, `.docx` fixes include the soft rules'
suggested rewrites alongside the mechanical ones. Plain text has no
accept/reject layer, so there `--fix` stays mechanical-only and the rewrites
need `--fix --unsafe`.

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
- `apps/website` — the website and documentation, built with [Blume](https://useblume.dev).
- `.github/workflows/ci.yml` — Rust checks plus the Bun/Blume website build.
- `.github/workflows/release.yml` — tagged CLI/desktop builds, R2 uploads, and release notes.

To work on the website locally: `bun install && bun run --cwd apps/website dev`
(needs Node 22.12+). The site is one Blume project:

- `apps/website/docs/` — the documentation, served at `/docs` via Blume's
  `basePath`. Markdown and MDX.
- `apps/website/pages/` — custom Astro pages Blume mounts at the site root: the
  landing page and the playground, plus redirect stubs holding the pre-Blume
  `/download`, `/changelog`, and `/rules/*` URLs open.
- `apps/website/blume.config.ts`, `theme.css`, `components.ts` — configuration,
  design tokens, and layout/MDX component overrides.
- `apps/website/scripts/` — generated docs pages, both gitignored:
  `generate-rule-docs.ts` writes the rule reference under `docs/rules/` from
  `crates/lawlint-core/builtin/rules/`, so a new rule documents itself;
  `generate-changelog.ts` writes `docs/changelog.mdx` from the repo's
  `CHANGELOG.md`, so a merged release-please PR publishes itself.

Both run from `bun run --cwd apps/website prepare:content`, which `dev` and
`build` invoke first.

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
