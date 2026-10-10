// Error types, Display strings identical to the Rust thiserror output.

export type LoadErrorKind =
  | "io"
  | "yaml"
  | "missing-field"
  | "invalid-field"
  | "invalid-regex"
  | "duplicate-id";

export class LoadError extends Error {
  readonly kind: LoadErrorKind;
  /** The file (or path) the error is about. */
  readonly file: string;
  readonly field?: string;
  readonly pattern?: string;
  constructor(
    kind: LoadErrorKind,
    message: string,
    file: string,
    extra?: { field?: string; pattern?: string },
  ) {
    super(message);
    this.name = "LoadError";
    this.kind = kind;
    this.file = file;
    this.field = extra?.field;
    this.pattern = extra?.pattern;
  }

  static io(path: string, source: string): LoadError {
    return new LoadError("io", `${path}: ${source}`, path);
  }
  static yaml(file: string, message: string): LoadError {
    return new LoadError("yaml", `${file}: invalid YAML: ${message}`, file);
  }
  static missingField(file: string, field: string): LoadError {
    return new LoadError("missing-field", `${file}: missing required field \`${field}\``, file, {
      field,
    });
  }
  static invalidField(file: string, field: string, message: string): LoadError {
    return new LoadError("invalid-field", `${file}: ${field}: ${message}`, file, { field });
  }
  static invalidRegex(file: string, field: string, pattern: string, message: string): LoadError {
    return new LoadError(
      "invalid-regex",
      `${file}: ${field}: invalid regex ${JSON.stringify(pattern)}: ${message}`,
      file,
      { field, pattern },
    );
  }
  static duplicateId(id: string, first: string, second: string): LoadError {
    return new LoadError(
      "duplicate-id",
      `duplicate rule id ${JSON.stringify(id)}: defined in ${first} and ${second}`,
      second,
    );
  }
}

export class JudgeError extends Error {
  readonly kind: "backend" | "malformed";
  constructor(kind: "backend" | "malformed", detail: string) {
    super(
      kind === "backend" ? `judge backend error: ${detail}` : `malformed judge response: ${detail}`,
    );
    this.name = "JudgeError";
    this.kind = kind;
  }
}
