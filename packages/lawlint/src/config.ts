import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import type { LintOptions } from "./types.js";

export function mergeOptions(config: Partial<LintOptions>, options: LintOptions = {}): LintOptions {
  return {
    ...config,
    ...options,
    severity: { ...config.severity, ...options.severity },
    thresholds: { ...config.thresholds, ...options.thresholds },
  };
}

export async function findConfig(cwd = process.cwd()): Promise<Partial<LintOptions>> {
  let directory = cwd;
  while (true) {
    try {
      const value = JSON.parse(
        await readFile(join(directory, "lawlint.config.json"), "utf8"),
      ) as Partial<LintOptions>;
      return value;
    } catch {
      /* continue upward */
    }
    const parent = dirname(directory);
    if (parent === directory) return {};
    directory = parent;
  }
}
