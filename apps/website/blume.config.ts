import { defineConfig } from "blume";

export default defineConfig({
  title: "lawlint",
  description:
    "A linter for AI-generated legal and general text. Flags the patterns that make prose sound machine-written, and suggests more human, direct alternatives.",

  // The docs are one surface of the main site, mounted under /docs. The landing
  // page, playground, download, and changelog are custom Astro pages in `pages/`
  // and stay at the site root.
  basePath: "/docs",

  logo: { text: "lawlint", href: "/" },

  // `navigation.featured` is not used for the site's other surfaces: Blume
  // prefixes `basePath` onto featured hrefs, so `/playground` would resolve to
  // `/docs/playground`. Those links live in the docs footer instead
  // (`components.ts` → `pages/_shared/DocsFooter.astro`).
  navigation: {
    sidebar: { display: "group" },
    repo: true,
  },

  theme: {
    // The site's oxblood-on-paper palette, carried into the docs so both
    // surfaces read as one brand.
    accent: "#7e2528",
    background: { light: "#f5f0e7", dark: "#17150f" },
    fonts: {
      body: "source-serif-4",
      display: "source-serif-4",
      mono: "ibm-plex-mono",
    },
    mode: "system",
    radius: "sm",
  },

  markdown: {
    code: { icons: true, wrap: false },
    codeBlocks: { theme: { light: "github-light", dark: "vesper" } },
    headingAnchors: true,
  },

  search: {
    provider: "orama",
    popular: [
      { href: "/quickstart", label: "Quickstart", icon: "rocket" },
      { href: "/reference/cli", label: "CLI reference", icon: "terminal" },
      { href: "/rules", label: "Rule reference", icon: "list" },
    ],
  },

  seo: {
    og: {
      enabled: true,
      // Custom-page OG cards only; docs pages take their card title from the
      // page itself. `/download` and `/changelog` are redirect stubs now.
      titles: {
        "/": "lawlint",
        "/playground": "Playground",
      },
    },
    sitemap: true,
    robots: true,
    structuredData: true,
  },

  github: { owner: "sethtjf", repo: "lawlint", branch: "main", dir: "apps/website" },

  // The pre-blume docs lived at flat `/docs/*` routes. Blume applies `basePath`
  // to a redirect's `from` as well as its `to`, so only routes that already sit
  // under /docs can be expressed here; the legacy `/rules/*` URLs baked into
  // every diagnostic's `docsUrl` are redirected by `pages/rules/*` instead.
  redirects: [
    { from: "/docs/getting-started", to: "/docs/quickstart" },
    { from: "/docs/cli", to: "/docs/reference/cli" },
    { from: "/docs/sdk", to: "/docs/reference/sdk" },
  ],

  feedback: false,
  lastModified: true,

  ai: { llmsTxt: true },

  deployment: { output: "static", site: "https://lawlint.com" },
});
