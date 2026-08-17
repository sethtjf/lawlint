import { spawnSync } from "node:child_process";

// These advisories are currently reachable only through Blume's build-time
// dependency graph. Keep the exceptions explicit and narrow: any new advisory
// (or a fix becoming available) must make this command fail until reviewed.
const ignored = [
  "GHSA-mh99-v99m-4gvg", // brace-expansion in Blume's EPUB/AsyncAPI build tooling
  "GHSA-2v37-7h3g-55p8", // nanoid 3 nested in Vite/PostCSS build tooling
  "GHSA-rgw5-rvv9-x895", // brace-expansion in epub-gen-memory/filelist
  "GHSA-2pvr-wf23-7pc7", // Astro 5 nested under Scalar's build adapter
  "GHSA-8hv8-536x-4wqp", // Astro 5 nested under Scalar's build adapter
  "GHSA-f88m-g3jw-g9cj", // sharp 0.34 nested under Scalar's build adapter
  "GHSA-4cwx-7wf7-3272", // undici 7 nested under Blume's build tooling
  "GHSA-w3rx-r6r6-pgpr", // image-size used by Blume's local asset build
  "GHSA-5p2g-fcmc-qvqq", // image-size used by Blume's local asset build
  "GHSA-9wv6-86v2-598j", // path-to-regexp in an unused Vercel adapter
  "GHSA-5p4m-2wfm-xmqj", // js-yaml 3 in AsyncAPI/gray-matter build tooling
];

const args = ["audit", "--audit-level=high"];
for (const advisory of ignored) args.push("--ignore", advisory);

const result = spawnSync("bun", args, { stdio: "inherit" });
if (result.error) throw result.error;
process.exit(result.status ?? 1);
