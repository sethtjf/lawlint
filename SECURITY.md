# Security policy

Report security issues privately to the repository maintainers rather than
opening a public issue with exploit details.

## Dependency audits

The active Rust workspace is checked with `cargo audit --deny warnings`. The
archived Tauri prototype is deliberately outside that workspace, so its GTK
dependency tree cannot affect the CLI, WASM, or website release artifacts.

The website uses Blume for documentation generation. `bun run audit:security`
runs Bun's high-severity audit (`--audit-level=high`), so moderate and low
advisories are intentionally outside this gate. It fails on every high or
critical advisory except the explicit IDs listed in `scripts/audit-bun.mjs`;
the pinned Bun version supports the repeated `--ignore` flags used there.
Those exceptions are transitive, build-time packages in Blume's graph (the
Vercel adapter, Scalar's Astro 5 peer, EPUB generation, AsyncAPI/static
metadata parsing, Vite/PostCSS, or local image processing); the Cloudflare
deployment does not accept user-supplied documents or image archives through
those paths. `image-size` has no fixed npm release as of this policy.

Review the exception list whenever Blume or the website toolchain changes. A
new fixed release should be adopted and its exception removed before merging.

The root Bun manifest intentionally overrides `ip-address` to `10.5.0`. Some
Blume build tooling still requests the older `^9` range, but the release build
and website checks exercise the pinned graph successfully. Revisit this
override when those consumers support the current major cleanly, and remove it
only after the lockfile and the full website build remain green.

Native installers use the unsigned `latest/VERSION` object as a release
pointer, then verify the selected immutable archive against that release's
`SHA256SUMS`. This protects downloads from transit corruption and mixed-release
publication windows, but it is not a cryptographic anti-rollback guarantee:
TLS and control of the distribution bucket remain part of the trust boundary.
