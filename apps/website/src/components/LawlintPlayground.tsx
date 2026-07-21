import init, { lint, remediationPrompt } from "@/generated/wasm/lawlint_wasm.js";
import { type DragEvent, type UIEvent, useEffect, useMemo, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";

type Diagnostic = {
  ruleId: string;
  severity: string;
  message: string;
  suggestion?: string;
  line: number;
  column: number;
  endLine?: number;
  endColumn?: number;
};

type RangedDiagnostic = Diagnostic & { start: number; end: number };

type Stats = {
  score: number;
  wordCount: number;
  sentenceCount: number;
};

let wasmReady: ReturnType<typeof init> | undefined;

function getWasmReady() {
  wasmReady ??= init();
  return wasmReady;
}

const SAMPLE = `It is important to note that in today's fast-paced world, legal teams must delve into the landscape of regulatory obligations. Moreover, the aforementioned parties shall, pursuant to Section 4(b), cease and desist any and all conduct hereinafter described.

Moreover, this Agreement is not only binding but also enforceable. Moreover, it could be said that the obligations herein are arguably very significant — indeed, they are crucially important.

The parties agree. The parties covenant. The parties acknowledge that this arrangement — negotiated, drafted, and executed in good faith — shall be deemed accepted, notwithstanding the foregoing.`;

function escapeHtml(text: string) {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function lineStartOffsets(text: string) {
  const offsets = [0];
  for (let index = 0; index < text.length; index += 1) {
    if (text[index] === "\n") offsets.push(index + 1);
  }
  return offsets;
}

function toRange(diagnostic: Diagnostic, starts: number[], length: number) {
  const start = (starts[diagnostic.line - 1] ?? 0) + diagnostic.column - 1;
  const end =
    diagnostic.endLine !== undefined && diagnostic.endColumn !== undefined
      ? (starts[diagnostic.endLine - 1] ?? 0) + diagnostic.endColumn - 1
      : start + 1;
  return {
    start: Math.min(start, length),
    end: Math.min(Math.max(end, start + 1), length),
  };
}

function renderMarks(text: string, diagnostics: RangedDiagnostic[]) {
  const sorted = [...diagnostics].sort((a, b) => a.start - b.start || b.end - a.end);
  let cursor = 0;
  let html = "";

  for (const diagnostic of sorted) {
    if (diagnostic.end <= cursor) continue;
    const start = Math.max(diagnostic.start, cursor);
    html += escapeHtml(text.slice(cursor, start));
    html += `<mark class="mark-${diagnostic.severity}">${escapeHtml(text.slice(start, diagnostic.end))}</mark>`;
    cursor = diagnostic.end;
  }

  return `${html}${escapeHtml(text.slice(cursor))}\n`;
}

function reportBaseName(sourceName: string) {
  const withoutExtension = sourceName.replace(/\.[^/.]+$/, "");
  return (withoutExtension || "untitled").replace(/[^a-zA-Z0-9._-]+/g, "-");
}

function downloadReport(content: string, filename: string, type: string) {
  const url = URL.createObjectURL(new Blob([content], { type }));
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

async function writeClipboard(content: string) {
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(content);
      return true;
    } catch {
      // Fall through to the execCommand path below.
    }
  }

  const fallback = document.createElement("textarea");
  fallback.value = content;
  fallback.style.position = "fixed";
  fallback.style.opacity = "0";
  document.body.appendChild(fallback);
  fallback.select();
  try {
    return document.execCommand("copy");
  } finally {
    fallback.remove();
  }
}

function severityVariant(severity: string) {
  if (severity === "error") return "default" as const;
  if (severity === "warning") return "warning" as const;
  return "info" as const;
}

export default function LawlintPlayground() {
  const [text, setText] = useState("");
  const [markdown, setMarkdown] = useState(false);
  const [sourceName, setSourceName] = useState("untitled");
  const [dragging, setDragging] = useState(false);
  const [view, setView] = useState<"editor" | "preview">("editor");
  const [copied, setCopied] = useState(false);
  const [promptCopied, setPromptCopied] = useState(false);
  const [stats, setStats] = useState<Stats>({
    score: 100,
    wordCount: 0,
    sentenceCount: 0,
  });
  const [rangedDiagnostics, setRangedDiagnostics] = useState<RangedDiagnostic[]>([]);
  const [latestResult, setLatestResult] = useState<{
    stats: Stats;
    diagnostics: Diagnostic[];
  } | null>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const backdropRef = useRef<HTMLDivElement>(null);
  const copyTimerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const promptCopyTimerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  useEffect(() => {
    let cancelled = false;
    const timer = setTimeout(() => {
      void getWasmReady().then(() => {
        if (cancelled) return;
        const result = lint(text, markdown ? { markdown: true } : {});
        const diagnostics = result.diagnostics as Diagnostic[];
        const starts = lineStartOffsets(text);
        const ranged = diagnostics
          .map((diagnostic) => ({
            ...diagnostic,
            ...toRange(diagnostic, starts, text.length),
          }))
          .sort((a, b) => a.start - b.start);

        setStats(result.stats as Stats);
        setRangedDiagnostics(ranged);
        setLatestResult({
          stats: result.stats as Stats,
          diagnostics,
        });
      });
    }, 120);

    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [markdown, text]);

  const marks = useMemo(() => renderMarks(text, rangedDiagnostics), [rangedDiagnostics, text]);
  const hasText = Boolean(text.trim());

  function updateText(value: string) {
    setText(value);
    if (view === "preview") setView("editor");
  }

  function loadSample() {
    setText(SAMPLE);
    setMarkdown(false);
    setSourceName("untitled");
    setView("editor");
  }

  function clear() {
    setText("");
    setSourceName("untitled");
    setView("editor");
    inputRef.current?.focus();
  }

  async function loadFile(file: File) {
    updateText(await file.text());
    setMarkdown(/\.(md|markdown)$/i.test(file.name));
    setSourceName(file.name);
  }

  function selectDiagnostic(diagnostic: RangedDiagnostic) {
    const input = inputRef.current;
    if (!input) return;
    input.focus();
    input.setSelectionRange(diagnostic.start, diagnostic.end);
    requestAnimationFrame(() => {
      const lineHeight = Number.parseFloat(getComputedStyle(input).lineHeight) || 24;
      input.scrollTop = Math.max(0, (diagnostic.line - 3) * lineHeight);
      if (backdropRef.current) backdropRef.current.scrollTop = input.scrollTop;
    });
  }

  function markdownReport() {
    if (!latestResult) return "";
    const generatedAt = new Date().toISOString();
    const { diagnostics, stats: currentStats } = latestResult;
    const lines = [
      "# lawlint report",
      "",
      `- **Source:** ${sourceName.replace(/\r?\n/g, " ")}`,
      `- **Generated:** ${generatedAt}`,
      `- **Summary:** ${currentStats.score}/100 score · ${currentStats.wordCount} words · ${currentStats.sentenceCount} sentences · ${diagnostics.length} issues`,
      "",
      "## Diagnostics",
      "",
    ];

    if (diagnostics.length === 0) {
      lines.push("No issues found.");
    } else {
      rangedDiagnostics.forEach((diagnostic, index) => {
        lines.push(
          `${index + 1}. **${diagnostic.severity}** \`${diagnostic.ruleId}\` at \`${diagnostic.line}:${diagnostic.column}\``,
          `   - **Message:** ${diagnostic.message}`,
        );
        if (diagnostic.suggestion) {
          lines.push(`   - **Suggestion:** ${diagnostic.suggestion}`);
        }
        lines.push("");
      });
    }

    return `${lines.join("\n").trimEnd()}\n`;
  }

  function jsonReport() {
    if (!latestResult) return "";
    return JSON.stringify(
      {
        source: sourceName,
        generatedAt: new Date().toISOString(),
        options: { markdown },
        stats: latestResult.stats,
        diagnostics: latestResult.diagnostics,
      },
      null,
      2,
    );
  }

  async function copyJson() {
    const report = jsonReport();
    if (!report) return;
    if (!(await writeClipboard(report))) return;
    clearTimeout(copyTimerRef.current);
    setCopied(true);
    copyTimerRef.current = setTimeout(() => setCopied(false), 1400);
  }

  async function copyPrompt() {
    const prompt = remediationPrompt(text, markdown ? { markdown: true } : {}) as string | null;
    if (!prompt) return;
    if (!(await writeClipboard(prompt))) return;
    clearTimeout(promptCopyTimerRef.current);
    setPromptCopied(true);
    promptCopyTimerRef.current = setTimeout(() => setPromptCopied(false), 1400);
  }

  function handleScroll(event: UIEvent<HTMLTextAreaElement>) {
    if (!backdropRef.current) return;
    backdropRef.current.scrollTop = event.currentTarget.scrollTop;
    backdropRef.current.scrollLeft = event.currentTarget.scrollLeft;
  }

  function handleDrop(event: DragEvent<HTMLDivElement>) {
    event.preventDefault();
    setDragging(false);
    const file = event.dataTransfer.files[0];
    if (file) void loadFile(file);
  }

  return (
    <section className="shadcn-theme mx-auto max-w-[68rem] pb-8 pt-20 sm:pt-24">
      <div className="mb-10 max-w-3xl">
        <div className="font-mono text-xs uppercase tracking-[0.16em] text-primary">Playground</div>
        <h1 className="mt-4 max-w-[12ch] font-serif text-5xl font-semibold leading-[0.98] tracking-[-0.06em] text-foreground sm:text-7xl">
          Try lawlint in your browser.
        </h1>
        <p className="mt-6 max-w-2xl text-lg leading-8 text-muted-foreground">
          Paste a draft, drop in a file, or start from the sample. Everything runs locally in your
          browser — nothing you write leaves this page.
        </p>
      </div>

      <div className="mb-4 flex flex-wrap items-center gap-2">
        <Button onClick={loadSample} size="sm">
          Load sample
        </Button>
        <Button onClick={() => fileInputRef.current?.click()} size="sm" variant="outline">
          Open file…
        </Button>
        <input
          ref={fileInputRef}
          accept=".txt,.md,.markdown,text/plain,text/markdown"
          className="hidden"
          onChange={(event) => {
            const file = event.target.files?.[0];
            if (file) void loadFile(file);
            event.target.value = "";
          }}
          type="file"
        />
        <span
          className="max-w-40 truncate px-2 font-mono text-xs uppercase tracking-[0.08em] text-muted-foreground"
          title={sourceName}
        >
          {sourceName}
        </span>
        <div className="ml-auto flex flex-wrap items-center gap-2">
          <Button
            disabled={!hasText || !latestResult}
            onClick={() =>
              downloadReport(
                markdownReport(),
                `${reportBaseName(sourceName)}.lawlint.md`,
                "text/markdown;charset=utf-8",
              )
            }
            size="sm"
            variant="outline"
          >
            Download Markdown
          </Button>
          <Button
            disabled={!hasText || !latestResult}
            onClick={() =>
              downloadReport(
                jsonReport(),
                `${reportBaseName(sourceName)}.lawlint.json`,
                "application/json;charset=utf-8",
              )
            }
            size="sm"
            variant="outline"
          >
            Download JSON
          </Button>
          <Button
            disabled={!hasText || !latestResult}
            onClick={() => void copyJson()}
            size="sm"
            variant="outline"
          >
            {copied ? "Copied" : "Copy JSON"}
          </Button>
          <Button
            disabled={!latestResult || latestResult.diagnostics.length === 0}
            onClick={() => void copyPrompt()}
            size="sm"
            variant="outline"
          >
            {promptCopied ? "Copied" : "Copy AI prompt"}
          </Button>
          <Button onClick={clear} size="sm" variant="ghost">
            Clear
          </Button>
        </div>
      </div>

      <Card className="overflow-hidden border-border bg-card shadow-[0_18px_60px_rgba(33,30,27,0.08)]">
        <CardHeader className="border-b border-border bg-secondary/35 px-4 py-3 sm:px-5">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex items-center gap-3">
              <CardTitle className="font-mono text-xs font-medium uppercase tracking-[0.12em]">
                Draft workspace
              </CardTitle>
              <Separator className="h-4 w-px" orientation="vertical" />
              <label className="flex items-center gap-2 font-mono text-xs uppercase tracking-[0.08em] text-muted-foreground">
                <input
                  checked={markdown}
                  className="accent-primary"
                  onChange={(event) => {
                    const checked = event.target.checked;
                    setMarkdown(checked);
                    if (!checked) setView("editor");
                  }}
                  type="checkbox"
                />
                Treat as Markdown
              </label>
            </div>
            <div className="flex items-center gap-1" role="tablist" aria-label="Draft view">
              <Button
                aria-selected={view === "editor"}
                onClick={() => setView("editor")}
                role="tab"
                size="sm"
                variant={view === "editor" ? "secondary" : "ghost"}
              >
                Editor
              </Button>
              <Button
                aria-selected={view === "preview"}
                disabled={!markdown}
                onClick={() => setView("preview")}
                role="tab"
                size="sm"
                variant={view === "preview" ? "secondary" : "ghost"}
              >
                Preview
              </Button>
            </div>
          </div>
        </CardHeader>

        <CardContent className="p-0">
          {view === "editor" ? (
            <div
              className={cn(
                "relative min-h-[28rem] border-b border-border bg-[var(--lawlint-surface)] sm:min-h-[34rem]",
                dragging &&
                  "outline outline-2 outline-dashed outline-primary outline-offset-[-7px]",
              )}
              onDragEnter={(event) => {
                event.preventDefault();
                setDragging(true);
              }}
              onDragLeave={() => setDragging(false)}
              onDragOver={(event) => event.preventDefault()}
              onDrop={handleDrop}
            >
              <div
                ref={backdropRef}
                className="pointer-events-none absolute inset-0 overflow-hidden"
              >
                <div
                  className="whitespace-pre-wrap break-words px-5 py-5 font-mono text-[0.9rem] leading-7 text-transparent"
                  // biome-ignore lint/security/noDangerouslySetInnerHtml: markup is generated from escaped editor text
                  dangerouslySetInnerHTML={{ __html: marks }}
                />
              </div>
              <Textarea
                ref={inputRef}
                aria-label="Text to lint"
                autoCapitalize="off"
                autoComplete="off"
                className="relative z-10 min-h-[28rem] resize-y rounded-none border-0 bg-transparent px-5 py-5 font-mono text-[0.9rem] leading-7 text-foreground shadow-none focus-visible:ring-0 sm:min-h-[34rem]"
                onChange={(event) => updateText(event.target.value)}
                onScroll={handleScroll}
                placeholder="Paste your text here, or drop a .txt / .md file…"
                spellCheck={false}
                value={text}
              />
              {dragging ? (
                <div className="pointer-events-none absolute inset-0 z-20 grid place-items-center bg-background/80">
                  <Badge className="px-4 py-2 text-sm" variant="default">
                    Drop a text file to open it
                  </Badge>
                </div>
              ) : null}
            </div>
          ) : (
            <ScrollArea className="min-h-[28rem] border-b border-border bg-background px-5 py-6 sm:min-h-[34rem] sm:px-8">
              {hasText ? (
                <div className="markdown-preview max-w-3xl space-y-5 text-[1rem] leading-8 text-foreground [&_a]:text-primary [&_a]:underline [&_blockquote]:border-l-2 [&_blockquote]:border-primary [&_blockquote]:pl-4 [&_code]:bg-secondary [&_code]:px-1 [&_code]:font-mono [&_code]:text-sm [&_h1]:font-serif [&_h1]:text-3xl [&_h1]:font-semibold [&_h2]:font-serif [&_h2]:text-2xl [&_h2]:font-semibold [&_h3]:font-serif [&_h3]:text-xl [&_h3]:font-semibold [&_hr]:border-border [&_li]:ml-5 [&_li]:list-disc [&_pre]:overflow-x-auto [&_pre]:bg-foreground [&_pre]:p-4 [&_pre]:font-mono [&_pre]:text-sm [&_pre]:text-background">
                  <ReactMarkdown remarkPlugins={[remarkGfm]}>{text}</ReactMarkdown>
                </div>
              ) : (
                <p className="font-mono text-sm text-muted-foreground">
                  Add Markdown text to preview it here.
                </p>
              )}
            </ScrollArea>
          )}
        </CardContent>
      </Card>

      <div className="mt-4 grid gap-4 lg:grid-cols-[minmax(0,1fr)_minmax(18rem,0.72fr)]">
        <Card className="border-border bg-card">
          <CardContent className="p-5">
            <div className="mb-4 flex items-end justify-between gap-4">
              <div>
                <div className="font-mono text-[0.68rem] uppercase tracking-[0.12em] text-muted-foreground">
                  Human-likeness score
                </div>
                <div className="mt-1 font-serif text-5xl font-semibold tracking-[-0.05em] text-foreground">
                  {hasText ? stats.score : "–"}
                  <span className="ml-1 text-xl font-normal text-muted-foreground">/100</span>
                </div>
              </div>
              <div className="grid grid-cols-3 gap-4 text-right">
                <Stat label="Words" value={stats.wordCount} />
                <Stat label="Sentences" value={stats.sentenceCount} />
                <Stat label="Issues" value={rangedDiagnostics.length} />
              </div>
            </div>
            <Separator />
            <div className="mt-4 flex items-center justify-between gap-3">
              <span className="font-mono text-xs uppercase tracking-[0.1em] text-muted-foreground">
                {hasText
                  ? `${rangedDiagnostics.length} diagnostics in document order`
                  : "Waiting for a draft"}
              </span>
              {hasText && rangedDiagnostics.length === 0 ? (
                <Badge variant="secondary">No issues found</Badge>
              ) : null}
            </div>
          </CardContent>
        </Card>

        <Card className="border-border bg-card">
          <CardHeader className="px-5 pb-3 pt-5">
            <CardTitle className="font-mono text-xs font-medium uppercase tracking-[0.12em]">
              Diagnostics
            </CardTitle>
          </CardHeader>
          <CardContent className="px-5 pb-5 pt-0">
            <ScrollArea className="max-h-64 pr-1">
              {rangedDiagnostics.length === 0 ? (
                <p className="font-mono text-xs leading-6 text-muted-foreground">
                  {hasText ? "✓ No issues found." : "Diagnostics will appear here as you type."}
                </p>
              ) : (
                <div className="space-y-2">
                  {rangedDiagnostics.map((diagnostic, index) => (
                    <button
                      className="block w-full border border-border bg-background p-3 text-left transition-colors hover:border-primary"
                      key={`${diagnostic.ruleId}-${diagnostic.line}-${diagnostic.column}-${index}`}
                      onClick={() => selectDiagnostic(diagnostic)}
                      type="button"
                    >
                      <span className="flex items-center gap-2 font-mono text-[0.68rem] uppercase tracking-[0.06em]">
                        <Badge variant={severityVariant(diagnostic.severity)}>
                          {diagnostic.severity}
                        </Badge>
                        <a
                          className="text-foreground underline-offset-2 hover:text-primary hover:underline"
                          href={`/rules/${encodeURIComponent(diagnostic.ruleId)}`}
                          onClick={(event) => event.stopPropagation()}
                        >
                          {diagnostic.ruleId}
                        </a>
                        <span className="ml-auto text-muted-foreground">
                          {diagnostic.line}:{diagnostic.column}
                        </span>
                      </span>
                      <span className="mt-2 block text-sm leading-6 text-foreground">
                        {diagnostic.message}
                      </span>
                      {diagnostic.suggestion ? (
                        <span className="mt-1 block text-xs leading-5 text-muted-foreground">
                          → {diagnostic.suggestion}
                        </span>
                      ) : null}
                    </button>
                  ))}
                </div>
              )}
            </ScrollArea>
          </CardContent>
        </Card>
      </div>
    </section>
  );
}

function Stat({ label, value }: { label: string; value: number }) {
  return (
    <div>
      <div className="font-serif text-xl font-semibold text-foreground">{value}</div>
      <div className="font-mono text-[0.6rem] uppercase tracking-[0.08em] text-muted-foreground">
        {label}
      </div>
    </div>
  );
}
