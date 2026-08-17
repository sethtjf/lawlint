# Security policy

Report security issues privately to the repository maintainers rather than
opening a public issue with exploit details.

## Dependency audits

The active Rust workspace is checked with `cargo audit --deny warnings`. The
archived Tauri prototype is deliberately outside that workspace, so its GTK
dependency tree cannot affect the CLI, WASM, or website release artifacts.

The website uses Blume for documentation generation. `bun run audit:security`
runs Bun's high-severity audit and fails on every advisory except the explicit
IDs listed in `scripts/audit-bun.mjs`. Those exceptions are transitive,
build-time packages in Blume's graph (the Vercel adapter, Scalar's Astro 5
peer, EPUB generation, AsyncAPI/static metadata parsing, Vite/PostCSS, or local
image processing); the Cloudflare deployment does not accept user-supplied
documents or image archives through those paths. `image-size` has no fixed npm
release as of this policy.

Review the exception list whenever Blume or the website toolchain changes. A
new fixed release should be adopted and its exception removed before merging.
