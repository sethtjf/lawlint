// Lint options — same camelCase JSON the wasm binding accepts.

import type { Severity } from "./types";

export interface LintOptions {
  enable?: string[];
  disable?: string[];
  severity?: Record<string, Severity>;
  thresholds?: Record<string, number>;
  markdown?: boolean;
  ruleDirs?: string[];
  judge?: JudgeOptions;
  ai?: AiOptions;
  learn?: LearnOptions;
}

export interface AiOptions {
  model?: string;
  features?: Record<string, string>;
}

export interface JudgeOptions {
  enabled?: boolean;
  model?: string;
  floor?: number;
  maxTokens?: number;
  concurrency?: number;
  contextChars?: number;
  perRule?: boolean;
}

export interface LearnOptions {
  concurrency?: number;
}

/** The preferred model spec for `feature`: per-feature override, else default. */
export function aiModel(options: LintOptions, feature: string): string | undefined {
  const ai = options.ai;
  if (!ai) return undefined;
  return ai.features?.[feature] ?? ai.model;
}

/** Accept the same camelCase JSON the wasm binding accepts. */
export function parseLintOptions(json: unknown): LintOptions {
  if (json == null || typeof json !== "object") return {};
  return json as LintOptions;
}
