import { describe, expect, test } from "bun:test";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { parse as parseYaml } from "yaml";
import { InvalidRegexError, compileRustRegex } from "../src/regex";

const REPO = join(import.meta.dir, "../../..");

function rulePatterns(dir: string): string[] {
  const out: string[] = [];
  for (const file of readdirSync(dir)) {
    if (!file.endsWith(".md")) continue;
    const text = readFileSync(join(dir, file), "utf8");
    const match = text.match(/^---\n([\s\S]*?)\n---/);
    if (!match) continue;
    const meta = parseYaml(match[1]);
    for (const p of meta.patterns ?? []) {
      if (typeof p.pattern === "string") out.push(p.pattern);
    }
    if (typeof meta.allow_context?.pattern === "string") out.push(meta.allow_context.pattern);
  }
  return out;
}

const BUILTIN = rulePatterns(join(REPO, "crates/lawlint-core/builtin/rules"));
const SIMPLE = rulePatterns(join(REPO, "packages/simple-english/rules"));

// Regexes embedded in lawlint-core Rust source; they go through the same
// translator so \w etc. stay Unicode-aware.
const ENGINE_PATTERNS = [
  String.raw`(?i)^\d+(d|st|nd|rd|th)$`, // segment.rs ORDINAL_RE
  String.raw`^\(?([0-9]{1,3}|[A-Za-z]|[ivxlcIVXLC]{1,3})\)?$`, // segment.rs ENUM_RE
  String.raw`\d+\s+[A-Z][A-Za-z.]{0,15}\s+\d+`, // segment.rs REPORTER_RE
  String.raw`[A-Z][\w.'’-]*\s+vs?\.\s+[A-Z]`, // segment.rs CASE_STYLE_RE
  String.raw`(?u)\b[\w'’-]+\b`, // scoring.rs word regex
];

describe("rule corpus patterns", () => {
  test("every builtin rule pattern compiles", () => {
    expect(BUILTIN.length).toBeGreaterThan(150);
    for (const p of BUILTIN) {
      expect(() => compileRustRegex(p), p).not.toThrow();
    }
  });
  test("every simple-english pattern compiles", () => {
    expect(SIMPLE.length).toBeGreaterThan(0);
    for (const p of SIMPLE) {
      expect(() => compileRustRegex(p), p).not.toThrow();
    }
  });
  test("engine patterns compile", () => {
    for (const p of ENGINE_PATTERNS) {
      expect(() => compileRustRegex(p), p).not.toThrow();
    }
  });
});

describe("translation semantics", () => {
  test("\\b is Unicode-aware", () => {
    const re = compileRustRegex(String.raw`\bdelve\b`);
    expect(re.test("delve")).toBe(true);
    expect(re.test("delves")).toBe(false);
    // Unicode letters count as word chars, like Rust.
    expect(re.test("édelve")).toBe(false);
    expect(re.test("_delve")).toBe(false);
    expect(re.test(" delve ")).toBe(true);
  });
  test("\\w matches Unicode letters, marks, digits, connector punctuation", () => {
    const re = compileRustRegex(String.raw`^\w+$`);
    expect(re.test("café_1")).toBe(true);
    expect(re.test("a-b")).toBe(false);
    expect(compileRustRegex(String.raw`[\w'-]+`).test("née-s")).toBe(true);
  });
  test("\\d \\s are Unicode-aware; negations work outside classes", () => {
    expect(compileRustRegex(String.raw`^\d+$`).test("١٢٣")).toBe(true);
    expect(compileRustRegex(String.raw`a\sb`).test("a\u00A0b")).toBe(true);
    expect(compileRustRegex(String.raw`^\W+$`).test("!!!")).toBe(true);
    expect(compileRustRegex(String.raw`^\S+$`).test("x")).toBe(true);
  });
  test("(?i) maps to JS i flag", () => {
    const re = compileRustRegex(String.raw`(?i)\bdelve\b`);
    expect(re.test("DELVE")).toBe(true);
    expect(re.flags.includes("i")).toBe(true);
  });
  test("(?is) makes . match newlines", () => {
    const re = compileRustRegex(String.raw`(?is)a.b`);
    expect(re.test("a\nb")).toBe(true);
    // Without s, . does not match \n.
    expect(compileRustRegex("a.b").test("a\nb")).toBe(false);
  });
  test("(?m) treats only \\n as line terminator without the JS m flag", () => {
    const re = compileRustRegex(String.raw`(?m)^x$`, "g");
    expect("a\nx\nb".match(re)).toEqual(["x"]);
    // JS m would also treat \r as a terminator context; Rust does not.
    expect("a\rx\rb".match(re)).toBeNull();
    // Without m, ^/$ anchor the whole input.
    expect(compileRustRegex("^x$").test("x\nx")).toBe(false);
  });
  test("rejections mirror the Rust regex crate", () => {
    expect(() => compileRustRegex("a(?=b)")).toThrow(InvalidRegexError);
    expect(() => compileRustRegex("a(?!b)")).toThrow(InvalidRegexError);
    expect(() => compileRustRegex("(a)\\1")).toThrow(InvalidRegexError);
    expect(() => compileRustRegex("(?i:a)b")).toThrow(InvalidRegexError);
    expect(() => compileRustRegex("a(?i)b")).toThrow(InvalidRegexError);
    expect(() => compileRustRegex(String.raw`[^\D]`)).toThrow(InvalidRegexError);
  });
  test("leading flag group at position zero only", () => {
    const re = compileRustRegex(String.raw`(?im)^a+$`);
    expect(re.test("A")).toBe(true);
  });
  test("negated escapes inside classes union as complements", () => {
    const re = compileRustRegex(String.raw`(?is)\bnot only\b[\s\S]{0,120}\bbut also\b`);
    expect(re.test("not only\nthis\nbut also")).toBe(true);
    expect(compileRustRegex(String.raw`[\D]`).test("x")).toBe(true);
  });
  test("literal specials inside classes stay literal", () => {
    const re = compileRustRegex(String.raw`[''’]s`);
    expect(re.test("it's".replace("'", "’"))).toBe(true);
  });
});
