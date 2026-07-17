# lawlint

[![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

`lawlint` flags patterns that can make legal and general prose sound machine-generated,
and offers practical suggestions for more human, direct writing.

## Quickstart

```sh
bun install
bun run build
```

Run the documentation website locally:

```sh
bun run --cwd apps/website dev
bun run --cwd apps/website build
```

Build and run the native CLI:

```sh
cargo build --release -p lawlint-cli
./target/release/lawlint document.txt
cat document.md | ./target/release/lawlint - --format json
```

One-line installers and a download page are planned for the next release.

## Rust SDK

```rust
use lawlint_core::{lint, LintOptions};

let result = lint(
    "It is important to note that we delve into the landscape.",
    &LintOptions::default(),
);
```

`lawlint-core` is a pure Rust SDK and does not perform file or stdin I/O. The
website's [playground](https://lawlint.dev/playground) uses the same engine
compiled to WebAssembly.

## Monorepo layout

- `crates/lawlint-core` — pure-Rust linting SDK.
- `crates/lawlint-cli` — native CLI.
- `crates/lawlint-wasm` — browser binding used by the playground.
- `apps/website` — Astro documentation website with generated rule reference pages.
- `.github/workflows/ci.yml` — Rust checks plus the Bun/Astro website build.

The CLI discovers `lawlint.config.json` from the current directory upward.

## License

MIT. See [LICENSE](LICENSE).
