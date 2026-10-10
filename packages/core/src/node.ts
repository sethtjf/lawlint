// Node-only entry for @lawlint/core: filesystem rule loading.

import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { LoadError } from "./error";
import { parseManifest, parseRule } from "./loader";
import type { RuleDef } from "./loader";
import { RuleSet } from "./registry";

/** Load a package directory (style.yaml + rules/*.md). */
export function loadDir(path: string): RuleSet {
  const manifestPath = join(path, "style.yaml");
  let manifestText: string;
  try {
    manifestText = readFileSync(manifestPath, "utf8");
  } catch (e) {
    throw LoadError.io(manifestPath, e instanceof Error ? e.message : String(e));
  }
  const manifest = parseManifest(manifestPath, manifestText);
  const rules: [string, RuleDef][] = [];
  const rulesDir = join(path, "rules");
  let isDir = false;
  try {
    isDir = statSync(rulesDir).isDirectory();
  } catch {
    isDir = false;
  }
  if (isDir) {
    for (const name of readdirSync(rulesDir).sort()) {
      if (!name.endsWith(".md")) continue;
      const p = join(rulesDir, name);
      rules.push([p, parseRule(p, readFileSync(p, "utf8"))]);
    }
  }
  return RuleSet.fromParts(manifest, rules);
}
