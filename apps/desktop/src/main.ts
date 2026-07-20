import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { readTextFile } from "@tauri-apps/plugin-fs";
import "./styles.css";

type Diagnostic = {
  ruleId: string; // namespaced "package/name", e.g. "core/no-semicolons"
  severity: "error" | "warning" | "suggestion";
  tier: "static" | "statistical" | "inferential";
  intent: "style" | "detection"; // only detection findings move the score
  span: { start: number; end: number };
  message: string;
  suggestion?: string;
  confidence?: number; // tier-3 only
  line: number;
  column: number;
};

// Tolerate legacy "info" payloads; the v2 engine emits "suggestion".
function severityLabel(severity: string): string {
  return severity === "info" ? "suggestion" : severity;
}

type LintResult = {
  diagnostics: Diagnostic[];
  stats: { wordCount: number; sentenceCount: number; score: number };
};

type DocxFixSummary = {
  applied: number;
  skipped: number;
  outputPath: string;
};

const input = document.querySelector<HTMLTextAreaElement>("#input")!;
const editor = document.querySelector<HTMLElement>("#editor")!;
const markdownToggle = document.querySelector<HTMLInputElement>("#markdown-toggle")!;
const diagnostics = document.querySelector<HTMLElement>("#diagnostics")!;
const score = document.querySelector<HTMLElement>("#score")!;
const statWords = document.querySelector<HTMLElement>("#stat-words")!;
const statSentences = document.querySelector<HTMLElement>("#stat-sentences")!;
const statIssues = document.querySelector<HTMLElement>("#stat-issues")!;
const charCount = document.querySelector<HTMLElement>("#char-count")!;
const applyDocxButton = document.querySelector<HTMLButtonElement>("#apply-docx")!;
const docxStatus = document.querySelector<HTMLElement>("#docx-status")!;

// Path of the currently loaded .docx, or null for plain text / pasted input.
// A loaded docx is read-only in the editor: fixes are written back as Word
// tracked changes, not by editing this projected text.
let currentDocx: string | null = null;

function setDocxMode(path: string | null) {
  currentDocx = path;
  applyDocxButton.hidden = path === null;
  markdownToggle.disabled = path !== null;
  input.readOnly = path !== null;
  docxStatus.textContent = path ? `Loaded ${path} (read-only — fixes apply as tracked changes)` : "";
}

const SAMPLE = `It is important to note that in today's fast-paced world, legal teams must delve into the landscape of regulatory obligations. Moreover, the aforementioned parties shall, pursuant to Section 4(b), cease and desist any and all conduct hereinafter described.

Moreover, this Agreement is not only binding but also enforceable. Moreover, it could be said that the obligations herein are arguably very significant — indeed, they are crucially important.

The parties agree. The parties covenant. The parties acknowledge that this arrangement — negotiated, drafted, and executed in good faith — shall be deemed accepted, notwithstanding the foregoing.`;

function escapeHtml(value: string) {
  return value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

function render(result: LintResult) {
  score.textContent = input.value.trim() ? String(result.stats.score) : "–";
  statWords.textContent = String(result.stats.wordCount);
  statSentences.textContent = String(result.stats.sentenceCount);
  statIssues.textContent = String(result.diagnostics.length);

  if (!result.diagnostics.length) {
    diagnostics.innerHTML = `<div class="empty-state good"><span class="empty-mark">✓</span><p>${input.value.trim() ? "No issues found." : "Diagnostics will appear here as you type."}</p></div>`;
    return;
  }

  diagnostics.innerHTML = result.diagnostics.map((diagnostic) => `
    <article class="diagnostic">
      <div class="diagnostic-head">
        <span class="badge sev-${severityLabel(diagnostic.severity)}">${severityLabel(diagnostic.severity)}</span>
        <span class="rule-id">${escapeHtml(diagnostic.ruleId)}</span>
        <span class="location">${diagnostic.line}:${diagnostic.column}</span>
      </div>
      <p class="diagnostic-message">${escapeHtml(diagnostic.message)}</p>
      ${diagnostic.suggestion ? `<p class="suggestion">${escapeHtml(diagnostic.suggestion)}</p>` : ""}
    </article>
  `).join("");
}

async function lintText() {
  charCount.textContent = `${input.value.length.toLocaleString()} characters`;
  try {
    const result = await invoke<LintResult>("lint", {
      text: input.value,
      options: markdownToggle.checked ? { markdown: true } : {},
    });
    render(result);
  } catch (error) {
    diagnostics.innerHTML = `<div class="empty-state error"><span class="empty-mark">!</span><p>${escapeHtml(String(error))}</p></div>`;
  }
}

let timer: ReturnType<typeof setTimeout> | undefined;
function scheduleLint() {
  clearTimeout(timer);
  timer = setTimeout(() => void lintText(), 120);
}

input.addEventListener("input", scheduleLint);
markdownToggle.addEventListener("change", () => void lintText());

document.querySelector("#load-sample")?.addEventListener("click", () => {
  setDocxMode(null);
  input.value = SAMPLE;
  markdownToggle.checked = false;
  void lintText();
});

document.querySelector("#clear")?.addEventListener("click", () => {
  setDocxMode(null);
  input.value = "";
  void lintText();
  input.focus();
});

document.querySelector("#open-file")?.addEventListener("click", async () => {
  const selected = await open({
    multiple: false,
    filters: [{ name: "Documents", extensions: ["txt", "md", "markdown", "docx"] }],
  });
  if (typeof selected !== "string") return;
  if (/\.docx$/i.test(selected)) {
    try {
      const text = await invoke<string>("extract_docx", { path: selected });
      input.value = text;
      markdownToggle.checked = false;
      setDocxMode(selected);
      await lintText();
    } catch (error) {
      setDocxMode(null);
      diagnostics.innerHTML = `<div class="empty-state error"><span class="empty-mark">!</span><p>${escapeHtml(String(error))}</p></div>`;
    }
    return;
  }
  setDocxMode(null);
  input.value = await readTextFile(selected);
  markdownToggle.checked = /\.(md|markdown)$/i.test(selected);
  await lintText();
});

applyDocxButton.addEventListener("click", async () => {
  if (!currentDocx) return;
  applyDocxButton.disabled = true;
  docxStatus.textContent = "Applying tracked changes…";
  try {
    const summary = await invoke<DocxFixSummary>("apply_docx_fixes", {
      path: currentDocx,
      options: {},
      author: null,
    });
    const skipped = summary.skipped ? `, ${summary.skipped} skipped (multi-run)` : "";
    docxStatus.textContent = `Wrote ${summary.applied} tracked change(s)${skipped} → ${summary.outputPath}`;
  } catch (error) {
    docxStatus.textContent = `Error: ${String(error)}`;
  } finally {
    applyDocxButton.disabled = false;
  }
});

editor.addEventListener("dragover", (event) => {
  event.preventDefault();
  editor.classList.add("dragging");
});
editor.addEventListener("dragleave", () => editor.classList.remove("dragging"));
editor.addEventListener("drop", (event) => {
  event.preventDefault();
  editor.classList.remove("dragging");
  const file = event.dataTransfer?.files[0];
  if (!file) return;
  if (/\.docx$/i.test(file.name)) {
    docxStatus.textContent = "Use “Open file” to load a .docx.";
    return;
  }
  setDocxMode(null);
  void file.text().then((text) => {
    input.value = text;
    markdownToggle.checked = /\.(md|markdown)$/i.test(file.name);
    return lintText();
  });
});

document.addEventListener("keydown", (event) => {
  if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
    event.preventDefault();
    void lintText();
  }
});

void lintText();
