// Statistical (metric) engine — mirrors engines/statistical.rs.

import type { Block, Document, Sentence } from "../document";
import { LoadError } from "../error";
import {
  type MetricName,
  type RuleDef,
  isDocumentLevel,
  metricAlternatives,
  metricFromString,
} from "../loader";
import { compileRustRegex, rustFindIter } from "../regex";
import type { Ctx, Interests, Rule, RuleMeta } from "../rule";
import type { Src } from "../utf8";

const MIN_RHYTHM_SENTENCES = 8;
const MIN_RATE_WORDS = 50;

function sentenceWords(s: Sentence): number {
  return s.tokens.filter((t) => t.kind === "word" || t.kind === "number").length;
}

function openerWord(s: Sentence, src: Src): string | undefined {
  const t = s.tokens.find((t) => t.kind === "word");
  return t === undefined ? undefined : src.slice(t.range.start, t.range.end);
}

function textBlocks(doc: Document): Block[] {
  return doc.blocks.filter((b) => b.kind !== "codeblock");
}

function mean(xs: number[]): number {
  return xs.reduce((a, b) => a + b, 0) / xs.length;
}

const TRIAD_RE = compileRustRegex(
  String.raw`(?i)\b\w+(?:\s+\w+){0,3},\s+\w+(?:\s+\w+){0,3},\s+and\s+\w+`,
  "g",
);
const PAIR_RE = compileRustRegex(String.raw`\b[a-z][a-z'’-]{2,}\s+and\s+[a-z][a-z'’-]{2,}\b`, "g");

export function metricValue(metric: MetricName, src: Src, doc: Document): number | undefined {
  const sentences = textBlocks(doc).flatMap((b) => b.sentences);
  const lengths = sentences.map(sentenceWords);
  const words = lengths.reduce((a, b) => a + b, 0);
  switch (metric) {
    case "sentence-length":
    case "repetitive-openers":
      return undefined;
    case "sentence-length-variance": {
      if (lengths.length < MIN_RHYTHM_SENTENCES) return undefined;
      const m = mean(lengths);
      return lengths.reduce((a, x) => a + (x - m) ** 2, 0) / lengths.length;
    }
    case "cadence-autocorrelation": {
      if (lengths.length < MIN_RHYTHM_SENTENCES) return undefined;
      const m = mean(lengths);
      const denom = lengths.reduce((a, x) => a + (x - m) ** 2, 0);
      if (denom === 0) return undefined;
      let num = 0;
      for (let i = 0; i + 1 < lengths.length; i++) num += (lengths[i] - m) * (lengths[i + 1] - m);
      return num / denom;
    }
    case "repeated-opener-density": {
      if (lengths.length < MIN_RHYTHM_SENTENCES) return undefined;
      let repeats = 0;
      let transitions = 0;
      for (const block of textBlocks(doc)) {
        const openers = block.sentences
          .map((s) => openerWord(s, src)?.toLowerCase())
          .filter((w): w is string => w !== undefined);
        for (let i = 0; i + 1 < openers.length; i++) {
          transitions += 1;
          if (openers[i] === openers[i + 1]) repeats += 1;
        }
      }
      if (transitions === 0) return undefined;
      return repeats / transitions;
    }
    case "triad-density":
    case "paired-adjective-rate": {
      if (words < MIN_RATE_WORDS) return undefined;
      const re = metric === "triad-density" ? TRIAD_RE : PAIR_RE;
      let count = 0;
      for (const s of sentences) {
        const slice = src.slice(s.range.start, s.range.end);
        for (const _ of rustFindIter(re, slice)) count += 1;
      }
      return (count / words) * 1000;
    }
  }
}

type Direction = "above" | "below";

function spelled(n: number): string {
  const names = ["", "", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine", "Ten"];
  return n < names.length ? names[n] : String(n);
}

const LABELS: Partial<Record<MetricName, string>> = {
  "sentence-length-variance": "Sentence-length variance",
  "cadence-autocorrelation": "Sentence-cadence autocorrelation",
  "repeated-opener-density": "Repeated-opener rate",
  "triad-density": "Triad density (per 1000 words)",
  "paired-adjective-rate": "Paired-conjunction rate (per 1000 words)",
};

export class StatisticalEngine implements Rule {
  private readonly meta_: RuleMeta;
  private readonly metric: MetricName;
  private readonly maxWords: number;
  private readonly runLength: number;
  private opener?: string;
  private run = 0;
  private readonly threshold: number;
  private readonly direction: Direction;

  private constructor(
    meta: RuleMeta,
    metric: MetricName,
    maxWords: number,
    runLength: number,
    threshold: number,
    direction: Direction,
  ) {
    this.meta_ = meta;
    this.metric = metric;
    this.maxWords = maxWords;
    this.runLength = runLength;
    this.threshold = threshold;
    this.direction = direction;
  }

  static fromDef(meta: RuleMeta, def: RuleDef, file: string): StatisticalEngine {
    const metricStr = def.metric;
    if (metricStr === undefined) throw LoadError.missingField(file, "metric");
    const metric = metricFromString(metricStr);
    if (metric === undefined) {
      throw LoadError.invalidField(
        file,
        "metric",
        `${JSON.stringify(metricStr)} is not a known metric — use ${metricAlternatives()}`,
      );
    }
    const param = (name: string) => def.params?.[name];
    const maxWords = param("max_words") ?? 45;
    if (!Number.isFinite(maxWords) || maxWords < 1) {
      throw LoadError.invalidField(
        file,
        "params.max_words",
        `${maxWords} is not a valid sentence length — use a positive number of words like 45`,
      );
    }
    const runLengthRaw = param("run_length") ?? 3;
    if (!Number.isFinite(runLengthRaw) || runLengthRaw < 1 || runLengthRaw % 1 !== 0) {
      throw LoadError.invalidField(
        file,
        "params.run_length",
        `${runLengthRaw} is not a valid run length — use a whole number of at least 1`,
      );
    }
    let threshold = 0;
    let direction: Direction = "above";
    if (isDocumentLevel(metric)) {
      if (def.threshold === undefined) throw LoadError.missingField(file, "threshold");
      threshold = def.threshold;
      if (!Number.isFinite(threshold)) {
        throw LoadError.invalidField(
          file,
          "threshold",
          `${threshold} is not a valid threshold — use a finite number`,
        );
      }
      if (def.direction === undefined) throw LoadError.missingField(file, "direction");
      if (def.direction !== "above" && def.direction !== "below") {
        throw LoadError.invalidField(
          file,
          "direction",
          `${JSON.stringify(def.direction)} is not a direction — use above or below`,
        );
      }
      direction = def.direction;
    }
    return new StatisticalEngine(
      meta,
      metric,
      maxWords,
      Math.trunc(runLengthRaw),
      threshold,
      direction,
    );
  }

  meta(): RuleMeta {
    return this.meta_;
  }
  interests(): Interests {
    if (isDocumentLevel(this.metric)) return { documentExit: true };
    return { sentences: true, blocks: true };
  }

  private resetRun() {
    this.opener = undefined;
    this.run = 0;
  }

  checkBlock(_b: Block, _ctx: Ctx): void {
    this.resetRun();
  }

  checkSentence(s: Sentence, ctx: Ctx): void {
    if (this.metric === "sentence-length") {
      const n = sentenceWords(s);
      const max = ctx.threshold(this.maxWords);
      if (n > max) {
        ctx.report({ span: s.range, message: `Sentence is ${n} words; consider shortening it.` });
      }
    } else if (this.metric === "repetitive-openers") {
      const word = openerWord(s, ctx.srcBytes)?.toLowerCase();
      if (word === undefined) {
        this.resetRun();
        return;
      }
      if (this.opener === word) {
        this.run += 1;
      } else {
        this.opener = word;
        this.run = 1;
      }
      if (this.run >= this.runLength) {
        ctx.report({
          span: s.range,
          message: `${spelled(this.runLength)} consecutive sentences begin with “${word}”.`,
        });
        this.resetRun();
      }
    }
  }

  onDocumentExit(doc: Document, ctx: Ctx): void {
    if (!isDocumentLevel(this.metric)) return;
    const value = metricValue(this.metric, ctx.srcBytes, doc);
    if (value === undefined) return;
    const threshold = ctx.threshold(this.threshold);
    const fires = this.direction === "above" ? value > threshold : value < threshold;
    if (!fires) return;
    const first = textBlocks(doc).flatMap((b) => b.sentences)[0];
    if (first === undefined) return;
    const humanSide = this.direction === "above" ? "below" : "above";
    const label = LABELS[this.metric] as string;
    ctx.report({
      span: first.range,
      message: `${label} is ${value.toFixed(2)} across the document; human legal prose typically sits ${humanSide} ${threshold.toFixed(2)}.`,
    });
  }
}
