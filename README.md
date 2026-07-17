# lawlint

[![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

`lawlint` flags patterns that can make legal and general prose sound machine-generated,
and offers practical suggestions for more human, direct writing.

## Quickstart

```sh
pnpm install
pnpm build
pnpm --filter lawlint exec lawlint document.txt
```

Run the documentation website locally:

```sh
pnpm --filter website dev
pnpm --filter website build
```

Use JSON output or stdin:

```sh
cat document.md | pnpm --filter lawlint exec lawlint - --format json
```

As an SDK:

```ts
import { lint, lintFile } from "lawlint";

const result = lint("It is important to note that we delve into the landscape.");
console.log(result.stats.score, result.diagnostics);
const fileResult = await lintFile("document.txt");
```

## Monorepo layout

- `packages/lawlint` — published TypeScript SDK and `lawlint` CLI.
- `apps/website` — Astro documentation website with generated rule reference pages.
- `.github/workflows/ci.yml` — Node 20/22 build, lint, typecheck, and test matrix.

Rules are modular and third-party rules can be supplied through SDK options. A
`lawlint.config.json` file is discovered from the current directory upward.

## License

MIT. See [LICENSE](LICENSE).
