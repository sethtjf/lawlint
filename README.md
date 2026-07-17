# lawlint

[![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

`lawlint` flags patterns that can make legal and general prose sound machine-generated,
and offers practical suggestions for more human, direct writing.

## Quickstart

```sh
curl -fsSL https://lawlint.com/install.sh | sh
lawlint document.txt
```

For Windows PowerShell:

```sh
irm https://lawlint.com/install.ps1 | iex
```

The [download page](https://lawlint.com/download) also has the unsigned desktop
app for macOS and Windows, plus direct CLI archives for every supported
platform. The installers place the CLI in a user-local bin directory and do
not send documents anywhere.

To work on the documentation website locally:

```sh
bun install
bun run --cwd apps/website dev
```

To build the CLI from source instead:

```sh
cargo build --release -p lawlint-cli
./target/release/lawlint document.txt
```

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
- `crates/lawlint-wasm` — browser binding used by the playground.
- `apps/website` — Astro documentation website with generated rule reference pages.
- `.github/workflows/ci.yml` — Rust checks plus the Bun/Astro website build.
- `.github/workflows/release.yml` — tagged CLI/desktop builds, R2 uploads, and release notes.

The CLI discovers `lawlint.config.json` from the current directory upward.

## Maintainer releases

Pushing a `v*` tag runs the release workflow. It publishes versioned and
`latest/` assets to the R2 bucket behind `https://downloads.lawlint.com`, then
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
