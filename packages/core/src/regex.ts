// Translates Rust `regex` crate patterns to JavaScript RegExp.
// Rust classes are Unicode-aware by default, so \w, \d, \s and \b are
// rewritten to Unicode property forms and the `u` flag is always set.

const WORD = "\\p{Alphabetic}\\p{M}\\p{Nd}\\p{Pc}\\p{Join_Control}";
const WORD_CLASS = `[${WORD}]`;

export class InvalidRegexError extends Error {
  readonly pattern: string;
  constructor(pattern: string, message: string) {
    super(`invalid regex ${JSON.stringify(pattern)}: ${message}`);
    this.name = "InvalidRegexError";
    this.pattern = pattern;
  }
}

interface Flags {
  i: boolean;
  m: boolean;
  s: boolean;
}

// Consumes leading whole-pattern flag groups like `(?i)` / `(?ims)` / `(?u)`.
// Scoped groups `(?i:...)` and mid-pattern flag groups are rejected, matching
// the Rust rules corpus (only leading forms occur).
function leadingFlags(pattern: string): { flags: Flags; body: string } {
  const flags = { i: false, m: false, s: false };
  let rest = pattern;
  for (;;) {
    const flagsGroup = rest.match(/^\(\?([imsux]+)\)/);
    if (flagsGroup) {
      for (const ch of flagsGroup[1]) {
        if (ch === "i") flags.i = true;
        else if (ch === "m") flags.m = true;
        else if (ch === "s") flags.s = true;
        else if (ch === "x") {
          throw new InvalidRegexError(pattern, "verbose mode is not supported");
        }
        // (?u) is the Rust default
      }
      rest = rest.slice(flagsGroup[0].length);
      continue;
    }
    if (/^\(\?[imsux-]+:/.test(rest)) {
      throw new InvalidRegexError(pattern, "scoped flag groups are not supported");
    }
    break;
  }
  return { flags, body: rest };
}

const POSIX_CLASSES: Record<string, string> = {
  alnum: "\\p{Alphabetic}\\p{Nd}",
  alpha: "\\p{Alphabetic}",
  digit: "\\p{Nd}",
  lower: "\\p{Lowercase_Letter}",
  punct: "\\p{Punctuation}",
  space: "\\p{White_Space}",
  upper: "\\p{Uppercase_Letter}",
  word: WORD,
  xdigit: "A-Fa-f0-9",
};

export function translateRustRegex(pattern: string): { source: string; flags: string } {
  const { flags, body } = leadingFlags(pattern);
  const out: string[] = [];
  let i = 0;
  let inClass = false;
  let classNegated = false;
  let classStart = 0;
  // Negated escapes (\W \D \S) inside a class can't be written literally in
  // JS u-mode; Rust unions them as complements, so the class is rewritten at
  // close as (?:positive-items|complement1|complement2|...).
  let classComplements: string[] = [];

  const fail = (msg: string): never => {
    throw new InvalidRegexError(pattern, msg);
  };

  while (i < body.length) {
    const ch = body[i];

    if (ch === "\\") {
      const next = body[i + 1];
      if (next === undefined) fail("trailing backslash");
      i += 2;
      switch (next) {
        case "w":
          out.push(inClass ? WORD : WORD_CLASS);
          break;
        case "d":
          out.push(inClass ? "\\p{Nd}" : "[\\p{Nd}]");
          break;
        case "s":
          out.push(inClass ? "\\p{White_Space}" : "[\\p{White_Space}]");
          break;
        case "W":
          if (inClass) {
            classComplements.push(`[^${WORD}]`);
          } else {
            out.push(`[^${WORD}]`);
          }
          break;
        case "D":
          if (inClass) {
            classComplements.push("[^\\p{Nd}]");
          } else {
            out.push("[^\\p{Nd}]");
          }
          break;
        case "S":
          if (inClass) {
            classComplements.push("[^\\p{White_Space}]");
          } else {
            out.push("[^\\p{White_Space}]");
          }
          break;
        case "b":
          if (inClass) fail("\\b inside a character class");
          out.push(`(?:(?<=${WORD_CLASS})(?!${WORD_CLASS})|(?<!${WORD_CLASS})(?=${WORD_CLASS}))`);
          break;
        case "B":
          if (inClass) fail("\\B inside a character class");
          out.push(`(?:(?<=${WORD_CLASS})(?=${WORD_CLASS})|(?<!${WORD_CLASS})(?!${WORD_CLASS}))`);
          break;
        case "A":
          out.push("^");
          break;
        case "z":
        case "Z":
          out.push("$");
          break;
        case "p":
        case "P": {
          // \p{...} or \pX — pass through; JS `u` supports the same property syntax.
          if (body[i] === "{") {
            const end = body.indexOf("}", i);
            if (end === -1) fail("unterminated \\p{...}");
            out.push(`\\${next}${body.slice(i, end + 1)}`);
            i = end + 1;
          } else {
            out.push(`\\${next}${body[i] ?? ""}`);
            i += 1;
          }
          break;
        }
        case "a":
          out.push("\\x07");
          break;
        case "x":
          // Rust \x{...} is JS \u{...}; \xHH passes through.
          if (body[i] === "{") {
            const end = body.indexOf("}", i);
            if (end === -1) fail("unterminated \\x{...}");
            out.push(`\\u${body.slice(i, end + 1)}`);
            i = end + 1;
          } else {
            out.push("\\x");
          }
          break;
        case "n":
        case "r":
        case "t":
        case "f":
        case "v":
        case "u":
          out.push(`\\${next}`);
          break;
        default:
          if (/[0-9]/.test(next)) fail("backreferences are not supported");
          if (/[A-Za-z]/.test(next)) fail(`unsupported escape \\${next}`);
          // Rust accepts \ before any non-alphanumeric char; JS u-mode only
          // allows identity escapes for its syntax characters.
          if (/^[\\^$.*+?(){}\[\]|/]$/.test(next) || (next === "-" && inClass)) {
            out.push(`\\${next}`);
          } else {
            out.push(next);
          }
          break;
      }
      continue;
    }

    if (ch === "(" && body[i + 1] === "?") {
      const after = body[i + 2];
      if (after === ":") {
        out.push("(?:");
        i += 3;
        continue;
      }
      if (
        after === "=" ||
        after === "!" ||
        (after === "<" && (body[i + 3] === "=" || body[i + 3] === "!"))
      ) {
        fail("lookaround is not supported");
      }
      // Rust named groups (?P<name>...) and (?<name>...) → JS (?<name>...).
      if (after === "<" || (after === "P" && body[i + 3] === "<")) {
        const nameStart = i + (after === "P" ? 4 : 3);
        const nameEnd = body.indexOf(">", nameStart);
        if (nameEnd === -1) fail("unterminated group name");
        const name = body.slice(nameStart, nameEnd);
        if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name))
          fail(`invalid group name ${JSON.stringify(name)}`);
        out.push(`(?<${name}>`);
        i = nameEnd + 1;
        continue;
      }
      // Bare mid-pattern flag-set `(?flags)` — Rust applies it to the rest
      // of the enclosing group; we approximate by applying it globally.
      const flagSet = body.slice(i).match(/^\(\?([imsu]+)\)/);
      if (flagSet) {
        for (const f of flagSet[1]) {
          if (f === "i") flags.i = true;
          else if (f === "m") flags.m = true;
          else if (f === "s") flags.s = true;
          else if (f === "u") {
            // Rust default; already emulated.
          }
        }
        i += flagSet[0].length;
        continue;
      }
      if (body[i] === "?x") fail("verbose mode is not supported");
      fail("unsupported group syntax");
    }

    if (ch === "[") {
      if (!inClass && body[i + 1] === ":" && body.slice(i).match(/^\[:[a-z]+:\]/)) {
        // POSIX class outside a class is invalid in Rust anyway; handle below.
      }
      if (inClass) {
        // Inside a class, `[` starts a nested class only as POSIX `[:name:]`.
        const m = body.slice(i).match(/^\[:(\^?)([a-z]+):\]/);
        if (m) {
          const mapped = POSIX_CLASSES[m[2]];
          if (!mapped) fail(`unsupported POSIX class [:${m[2]}:]`);
          if (m[1]) fail("negated POSIX classes are not supported");
          out.push(mapped);
          i += m[0].length;
          continue;
        }
        fail("nested character classes are not supported");
      }
      inClass = true;
      classNegated = false;
      classComplements = [];
      classStart = out.length;
      out.push("[");
      i += 1;
      // `[^` negation
      if (body[i] === "^") {
        classNegated = true;
        out.push("^");
        i += 1;
      }
      continue;
    }

    if (ch === "]" && inClass) {
      inClass = false;
      out.push("]");
      i += 1;
      if (classComplements.length > 0) {
        if (classNegated) fail("negated class containing \\W \\D \\S cannot be expressed");
        const cls = out.slice(classStart).join("");
        out.splice(classStart, out.length - classStart, `(?:${cls}|${classComplements.join("|")})`);
      }
      continue;
    }

    if (
      inClass &&
      ((ch === "&" && body[i + 1] === "&") || ((ch === "-" || ch === "~") && body[i + 1] === ch))
    ) {
      fail("class set operations (&&, --, ~~) are not supported");
    }

    if (ch === "." && !inClass) {
      out.push(flags.s ? "[^]" : "[^\\n]");
      i += 1;
      continue;
    }

    if ((ch === "^" || ch === "$") && !inClass && flags.m) {
      out.push(ch === "^" ? "(?<![^\\n])" : "(?![^\\n])");
      i += 1;
      continue;
    }

    out.push(ch);
    i += 1;
  }

  return { source: out.join(""), flags: flags.i ? "i" : "" };
}

// Compiles a Rust regex pattern to a JS RegExp. Always uses the `u` flag;
// `extraFlags` lets callers request `g` or `y`.
export function compileRustRegex(pattern: string, extraFlags = ""): RegExp {
  const { source, flags } = translateRustRegex(pattern);
  try {
    return new RegExp(source, `u${flags}${extraFlags}`);
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    throw new InvalidRegexError(pattern, msg);
  }
}
