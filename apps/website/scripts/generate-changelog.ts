/**
 * Generate the docs changelog page from CHANGELOG.md at the repo root.
 *
 * CHANGELOG.md is maintained by release-please: each merged release PR appends a
 * version block. This turns those blocks into a single MDX page built from
 * Blume's `<Update>` component, so the changelog gets the docs chrome — sidebar,
 * search, table of contents, copy-as-markdown — instead of being its own
 * bespoke page.
 *
 * Output is `docs/changelog.mdx`, which is generated and gitignored.
 * Run via `bun run prepare:content`.
 */

import { readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const ROOT = resolve(import.meta.dirname, "..");
const CHANGELOG = resolve(ROOT, "../../CHANGELOG.md");
const OUT = join(ROOT, "docs/changelog.mdx");

interface Section {
  title: string;
  items: string[];
}

interface Release {
  version: string;
  compareUrl: string | null;
  date: string | null;
  sections: Section[];
}

// Release-please version headings look like `## [0.3.0](compare-url) (2026-07-17)`
// (the first release in a repo may have no compare link).
const HEADING = /^##\s+(?:\[([^\]]+)\]\(([^)]+)\)|(\S+))(?:\s+\((\d{4}-\d{2}-\d{2})\))?/;

const parse = (raw: string): Release[] => {
  const releases: Release[] = [];
  for (const line of raw.split("\n")) {
    const heading = line.match(HEADING);
    if (heading) {
      releases.push({
        version: heading[1] ?? heading[3],
        compareUrl: heading[2] ?? null,
        date: heading[4] ?? null,
        sections: [],
      });
      continue;
    }
    const current = releases.at(-1);
    if (!current) {
      continue;
    }
    const section = line.match(/^###\s+(.+)/);
    if (section) {
      current.sections.push({ title: section[1].trim(), items: [] });
      continue;
    }
    const item = line.match(/^\*\s+(.+)/);
    if (item && current.sections.length > 0) {
      current.sections.at(-1)?.items.push(item[1].trim());
    }
  }
  return releases;
};

const formatDate = (date: string | null): string | null =>
  date
    ? new Date(`${date}T00:00:00Z`).toLocaleDateString("en-US", {
        day: "numeric",
        month: "long",
        timeZone: "UTC",
        year: "numeric",
      })
    : null;

/** Escape the characters MDX would otherwise read as expressions or tags. */
const mdx = (text: string): string => text.replace(/([{}<>])/g, "\\$1");

const attr = (value: string): string => `"${value.replace(/"/g, "&quot;")}"`;

const renderRelease = (release: Release): string => {
  const lines: string[] = [];
  const attrs = [`id=${attr(`v${release.version}`)}`, `label=${attr(`v${release.version}`)}`];

  const date = formatDate(release.date);
  if (date) {
    attrs.push(`description=${attr(date)}`);
  }
  if (release.compareUrl) {
    attrs.push(`href=${attr(release.compareUrl)}`);
  }
  if (release.sections.length > 0) {
    attrs.push(`tags={${JSON.stringify(release.sections.map((section) => section.title))}}`);
  }

  lines.push(`<Update ${attrs.join(" ")}>`);
  lines.push("");
  for (const section of release.sections) {
    // Bold rather than a heading: every release repeats the same handful of
    // section names, so as headings they'd fill the table of contents with a
    // dozen identical "Features" entries. The names also already appear as tags
    // on the entry's rail.
    lines.push(`**${mdx(section.title)}**`);
    lines.push("");
    for (const item of section.items) {
      lines.push(`- ${mdx(item)}`);
    }
    lines.push("");
  }
  lines.push("</Update>");
  lines.push("");
  return lines.join("\n");
};

const releases = parse(readFileSync(CHANGELOG, "utf-8"));

if (releases.length === 0) {
  throw new Error(`No releases parsed from ${CHANGELOG}`);
}

const page = `---
title: Changelog
description: Every lawlint release, with the pull request behind each change.
icon: scroll-text
sidebar:
  order: 8
---

Every release, with the pull request behind each change. Assembled automatically
from merged work — the same notes ship with each
[GitHub release](https://github.com/sethtjf/lawlint/releases).

${releases.map(renderRelease).join("\n")}`;

writeFileSync(OUT, page);

console.log(`[changelog] wrote ${releases.length} releases to docs/changelog.mdx`);
