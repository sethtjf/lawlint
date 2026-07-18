#!/usr/bin/env python3
"""Lint the website's own copy with lawlint (dogfooding).

Extracts user-facing prose from the built site, then runs lawlint over each
page. Reads the rendered HTML rather than the .astro sources so it lints what
visitors actually see, without markup, class names, or code samples.

Usage:
    bun run build            # produce apps/website/dist
    python3 scripts/lint-website-copy.py

The lawlint binary defaults to `cargo run -q -p lawlint-cli --`; override with
LAWLINT_BIN=/path/to/lawlint to use an installed build.
"""

from __future__ import annotations

import html
import json
import os
import re
import shlex
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DIST = os.path.join(ROOT, "apps", "website", "dist")

# Hand-authored pages. Rule detail pages are excluded: their prose comes from
# rule metadata, which `--rule-metadata` lints from the source of truth instead.
PAGES = {
    "index.html": "home",
    "download/index.html": "download",
    "playground/index.html": "playground",
    "changelog/index.html": "changelog",
    "docs/getting-started/index.html": "docs/getting-started",
    "docs/cli/index.html": "docs/cli",
    "docs/sdk/index.html": "docs/sdk",
    "rules/index.html": "rules",
}

# Chrome and code are not prose we write in sentences.
DROP_ELEMENTS = ("script", "style", "pre", "nav", "header", "footer", "code")
PROSE_TAGS = ("h1", "h2", "h3", "p", "li")

# Fragments shorter than this are labels and UI chrome, not sentences.
MIN_BLOCK_CHARS = 12


def lawlint_command() -> list[str]:
    override = os.environ.get("LAWLINT_BIN")
    if override:
        return shlex.split(override)
    return ["cargo", "run", "-q", "-p", "lawlint-cli", "--"]


def clean(fragment: str) -> str:
    text = re.sub(r"<[^>]+>", "", fragment)
    return re.sub(r"\s+", " ", html.unescape(text)).strip()


def extract_prose(path: str) -> list[str]:
    markup = open(path, encoding="utf-8").read()
    main = re.search(r"<main\b.*?>(.*)</main>", markup, flags=re.S)
    markup = main.group(1) if main else markup
    for tag in DROP_ELEMENTS:
        markup = re.sub(rf"<{tag}\b.*?</{tag}>", " ", markup, flags=re.S | re.I)

    blocks = []
    for tag in PROSE_TAGS:
        for match in re.finditer(rf"<{tag}\b[^>]*>(.*?)</{tag}>", markup, flags=re.S | re.I):
            text = clean(match.group(1))
            if len(text) >= MIN_BLOCK_CHARS:
                blocks.append(text)
    return blocks


def rule_metadata_prose() -> list[str]:
    """Rule descriptions are user-facing copy too; lint them from rules.json."""
    path = os.path.join(ROOT, "apps", "website", "src", "generated", "rules.json")
    if not os.path.exists(path):
        return []
    blocks = []
    for rule in json.load(open(path, encoding="utf-8")):
        meta = rule.get("meta", {})
        for field in ("description", "rationale", "message"):
            value = meta.get(field)
            if value and len(value) >= MIN_BLOCK_CHARS:
                blocks.append(value)
    return blocks


def lint(blocks: list[str], command: list[str]) -> dict:
    with tempfile.NamedTemporaryFile("w", suffix=".txt", encoding="utf-8", delete=False) as f:
        f.write("\n\n".join(blocks) + "\n")
        temp = f.name
    try:
        result = subprocess.run(
            [*command, temp, "--format", "json"],
            capture_output=True,
            text=True,
            cwd=ROOT,
        )
        if not result.stdout.strip():
            raise SystemExit(f"lawlint produced no output:\n{result.stderr}")
        return json.loads(result.stdout)
    finally:
        os.unlink(temp)


def main() -> int:
    if not os.path.isdir(DIST):
        print(f"error: {DIST} not found. Run `bun run build` first.", file=sys.stderr)
        return 2

    command = lawlint_command()
    targets = [(name, os.path.join(DIST, rel)) for rel, name in PAGES.items()]

    findings: list[tuple[str, dict]] = []
    rows: list[tuple[str, int, int]] = []

    for name, path in targets:
        if not os.path.exists(path):
            print(f"warning: {name} not built, skipping", file=sys.stderr)
            continue
        blocks = extract_prose(path)
        if not blocks:
            print(f"warning: no prose extracted from {name}", file=sys.stderr)
            continue
        report = lint(blocks, command)
        rows.append((name, report["stats"]["score"], len(report["diagnostics"])))
        findings.extend((name, d) for d in report["diagnostics"])

    metadata = rule_metadata_prose()
    if metadata:
        report = lint(metadata, command)
        rows.append(("rule metadata", report["stats"]["score"], len(report["diagnostics"])))
        findings.extend(("rule metadata", d) for d in report["diagnostics"])

    width = max(len(name) for name, _, _ in rows)
    for name, score, count in rows:
        status = "ok" if count == 0 else f"{count} issue{'' if count == 1 else 's'}"
        print(f"{name:<{width}}  {score:3}/100  {status}")

    if findings:
        print(f"\n{len(findings)} finding(s):\n")
        for name, d in findings:
            print(f"  {name} — {d['ruleId']} ({d['severity']})")
            print(f"    {d['message']}")
            if d.get("excerpt"):
                print(f"    {d['excerpt'][:100]}")
        return 1

    print(f"\nAll {len(rows)} sources clean.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
