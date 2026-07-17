#!/usr/bin/env node
import pc from "picocolors";
import { findConfig, mergeOptions } from "./config.js";
import { lint, lintFile } from "./index.js";

function arg(name: string) {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] : undefined;
}

async function main() {
  const input = process.argv[2] ?? "-";
  const format = arg("--format") ?? "pretty";
  const enable = arg("--rules")?.split(",").filter(Boolean);
  const disable = arg("--disable")?.split(",").filter(Boolean);
  const quiet = process.argv.includes("--quiet");
  const markdown = process.argv.includes("--markdown");
  const maxWarnings = Number(arg("--max-warnings") ?? Number.POSITIVE_INFINITY);
  const options = {
    ...(enable ? { enable } : {}),
    ...(disable ? { disable } : {}),
    ...(markdown ? { markdown: true } : {}),
  };
  const result =
    input === "-"
      ? lint(
          await new Promise<string>((resolve, reject) => {
            let data = "";
            process.stdin.setEncoding("utf8");
            process.stdin.on("data", (chunk: string) => {
              data += chunk;
            });
            process.stdin.on("end", () => resolve(data));
            process.stdin.on("error", reject);
          }),
          mergeOptions(await findConfig(), options),
        )
      : await lintFile(input, options);

  if (format === "json") {
    console.log(JSON.stringify(result, null, 2));
  } else if (!quiet) {
    if (result.diagnostics.length === 0) console.log(pc.green("✓ No issues found."));
    for (const diagnostic of result.diagnostics) {
      const color =
        diagnostic.severity === "error"
          ? pc.red
          : diagnostic.severity === "info"
            ? pc.cyan
            : pc.yellow;
      console.log(
        `${color(`${diagnostic.line}:${diagnostic.column}`)} ${pc.bold(diagnostic.ruleId)} ${diagnostic.message}`,
      );
      console.log(`  ${pc.dim(diagnostic.excerpt)}`);
      if (diagnostic.suggestion) console.log(`  ${pc.dim(`Suggestion: ${diagnostic.suggestion}`)}`);
    }
    console.log(
      `\nHuman-likeness score: ${result.stats.score}/100 (${result.stats.wordCount} words, ${result.stats.sentenceCount} sentences)`,
    );
  }
  const warnings = result.diagnostics.filter((d) => d.severity === "warning").length;
  process.exitCode =
    result.diagnostics.some((d) => d.severity === "error") || warnings > maxWarnings ? 1 : 0;
}

void main();
