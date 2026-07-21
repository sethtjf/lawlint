import { defineComponents } from "blume";

export default defineComponents({
  layout: {
    // Blume ships no default site footer — it's an injection point. Ours carries
    // the links back to the landing page and playground, which live outside
    // `basePath` and so can't be expressed in `navigation.featured` (see
    // blume.config.ts). It also carries the site's global chrome CSS; the header
    // wordmark is styled there rather than through the `Logo` slot, because
    // PageLayout doesn't forward layout overrides to its Header the way
    // RootLayout does — see the comment in SiteFooter.astro.
    Footer: "./pages/_shared/SiteFooter.astro",
  },
  mdx: {
    // Available to every .mdx page without an import. Used by
    // docs/installation.mdx.
    Downloads: "./pages/_shared/Downloads.astro",
    // Blume ships `Update` (the version-rail changelog entry) but wires it only
    // into its own generated changelog index, not the authoring component map.
    // docs/changelog.mdx is generated from CHANGELOG.md and uses it directly, so
    // it's registered here through a wrapper — see ChangelogEntry.astro.
    Update: "./pages/_shared/ChangelogEntry.astro",
  },
});
