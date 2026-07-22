#!/usr/bin/env bun
/**
 * Guard against commit messages that release-please silently cannot read.
 *
 * release-please parses every commit since the last release with
 * `@conventional-commits/parser`. A message it cannot parse is logged at debug
 * level and skipped — so a release simply never happens, with a green
 * workflow and no warning anywhere. That is how 0.9.0 was missed: a squash
 * body contained `cfg(not(target_arch = "wasm32"))`, and the nested paren is
 * invalid in the grammar.
 *
 * The trap is that **commit bodies matter**. This repo squashes with
 * `squash_merge_commit_message: COMMIT_MESSAGES`, so every commit body on a
 * branch ends up inside the message release-please parses. Backticks do not
 * protect anything — the grammar has no notion of code spans.
 *
 * Two modes:
 *
 *   --since <ref>            Every commit in <ref>..HEAD. Post-merge net on
 *                            main, so a bad message is red immediately rather
 *                            than discovered at release time.
 *   --squash-preview <base>  Reconstructs the message GitHub will create when
 *                            this branch is squash-merged, and parses that.
 *                            Pre-merge, which is where it is cheap to fix.
 *                            Needs --title (the PR title).
 *
 * Only commits that *look* conventional are enforced: a subject matching
 * `type(scope): …` is a commit meant to drive a release, so failing to parse
 * is a bug. A subject that never claimed to be conventional (a plain merge
 * commit) is left alone — flagging those would train everyone to ignore this.
 */

import { execFileSync } from "node:child_process";
import { parser } from "@conventional-commits/parser";

/** Types release-please is configured to understand. */
const TYPES = "feat|fix|perf|revert|docs|style|chore|refactor|test|ci|build";
const LOOKS_CONVENTIONAL = new RegExp(`^(${TYPES})(\\([^)]*\\))?!?: .`);

const git = (...args) =>
  execFileSync("git", args, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });

/** Whether `ref` resolves to a commit in this checkout. */
function resolves(ref) {
  try {
    execFileSync("git", ["rev-parse", "--verify", "--quiet", `${ref}^{commit}`], {
      stdio: "ignore",
    });
    return true;
  } catch {
    return false;
  }
}

/** Commits in `range`, newest first, as {sha, message}. */
function commits(range) {
  // \x1e between records, \x1f between fields: neither occurs in commit text.
  const raw = git("log", "--format=%H%x1f%B%x1e", range).split("\x1e");
  return raw
    .map((record) => record.replace(/^\n/, ""))
    .filter((record) => record.trim())
    .map((record) => {
      const [sha, message] = record.split("\x1f");
      return { sha, message: message.trimEnd() };
    });
}

/**
 * The message GitHub composes for a squash merge, under this repo's settings
 * (title = PR title, body = COMMIT_MESSAGES). One commit contributes its body
 * directly; several are listed as `* <subject>` followed by their bodies.
 */
function squashPreview(title, branchCommits, prNumber) {
  const oldestFirst = [...branchCommits].reverse();
  const body =
    oldestFirst.length === 1
      ? oldestFirst[0].message.split("\n").slice(1).join("\n").trim()
      : oldestFirst
          .map((commit) => {
            const [subject, ...rest] = commit.message.split("\n");
            return `* ${subject}\n\n${rest.join("\n").trim()}`.trim();
          })
          .join("\n\n");
  return `${title} (#${prNumber})\n\n${body}\n`;
}

/** `undefined` when the message is fine, else the parser's complaint. */
function parseFailure(message) {
  if (!LOOKS_CONVENTIONAL.test(message)) return undefined;
  try {
    parser(message);
    return undefined;
  } catch (error) {
    return error.message;
  }
}

/** Quote the line the parser named, so the fix is obvious rather than a hunt. */
function blame(message, failure) {
  const at = /at (\d+):(\d+)/.exec(failure ?? "");
  if (!at) return "";
  const [, lineNo, col] = at.map(Number);
  const line = message.split("\n")[lineNo - 1];
  if (line === undefined) return "";
  return `\n      ${line}\n      ${" ".repeat(Math.max(col - 1, 0))}^`;
}

const HELP = `Fix: reword that line. Nested parentheses are the usual cause —
  cfg(not(...)), unwrap_or_else(|| f(x)). Backticks do not help; the
  conventional-commit grammar has no code spans. Prose, or a single level of
  parens, parses fine.

  Left unfixed, release-please skips the commit and opens no release PR, with
  a green workflow and no warning.`;

function main() {
  const argv = process.argv.slice(2);
  const flag = (name) => {
    const i = argv.indexOf(name);
    return i === -1 ? undefined : argv[i + 1];
  };

  const since = flag("--since");
  const base = flag("--squash-preview");
  let checked = [];

  if (base) {
    const title = flag("--title");
    if (!title) {
      console.error("--squash-preview requires --title");
      process.exit(2);
    }
    const branchCommits = commits(`${base}..HEAD`);
    if (branchCommits.length === 0) {
      console.log("No commits on this branch; nothing to check.");
      return;
    }
    const message = squashPreview(title, branchCommits, flag("--pr") ?? "0");
    checked = [{ sha: "squash-preview", message }];
  } else if (since) {
    // The release tag is created by release-please *during* the same workflow
    // run this check rides along in, so on a release merge the manifest names
    // a version whose tag does not exist yet. That is a race, not a problem
    // with any commit — failing here would make every release red and teach
    // everyone to ignore this job.
    if (!resolves(since)) {
      console.log(`${since} does not exist yet; nothing to check.`);
      return;
    }
    checked = commits(`${since}..HEAD`);
    if (checked.length === 0) {
      console.log(`No commits since ${since}; nothing to check.`);
      return;
    }
  } else {
    console.error(
      "usage: check-commit-messages.mjs (--since <ref> | --squash-preview <base> --title <t> [--pr <n>])",
    );
    process.exit(2);
  }

  const failures = checked
    .map((commit) => ({ ...commit, failure: parseFailure(commit.message) }))
    .filter((commit) => commit.failure);

  for (const { sha, message, failure } of failures) {
    const subject = message.split("\n")[0];
    const where = /^[0-9a-f]{40}$/.test(sha) ? sha.slice(0, 7) : sha;
    console.error(
      `\n✖ release-please cannot parse this commit message` +
        `\n  commit:  ${where}` +
        `\n  subject: ${subject}` +
        `\n  error:   ${failure}${blame(message, failure)}`,
    );
  }

  if (failures.length > 0) {
    console.error(`\n${HELP}\n`);
    process.exit(1);
  }

  console.log(
    base
      ? "✔ the squash-merge preview parses as a conventional commit."
      : `✔ ${checked.length} commit(s) parse as conventional commits.`,
  );
}

main();
