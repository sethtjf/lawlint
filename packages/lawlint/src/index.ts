import { readFile } from "node:fs/promises";
import { lint } from "./browser.js";
import { findConfig, mergeOptions } from "./config.js";
import type { LintOptions, LintResult } from "./types.js";
export * from "./browser.js";
export { findConfig, mergeOptions } from "./config.js";

export async function lintFile(path: string, options: LintOptions = {}): Promise<LintResult> {
  const config = await findConfig(options.cwd);
  const text = await readFile(path, "utf8");
  const markdown = path.toLowerCase().endsWith(".md");
  return lint(text, mergeOptions(config, { ...options, markdown: options.markdown ?? markdown }));
}
