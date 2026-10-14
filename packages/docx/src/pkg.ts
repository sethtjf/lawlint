// Read/write the OPC (zip) container while preserving every part. Untouched
// parts round-trip content-identical — the whole point of not going through
// a lossy typed model.

import { unzipSync, zipSync } from "fflate";
import { DocxError } from "./index";

interface Part {
  name: string;
  data: Uint8Array;
}

const textDecoder = new TextDecoder("utf-8", { fatal: true });
const textEncoder = new TextEncoder();

export class Package {
  parts: Part[] = [];

  static read(bytes: Uint8Array): Package {
    let entries: Record<string, Uint8Array>;
    try {
      entries = unzipSync(bytes);
    } catch (e) {
      throw new DocxError("zip", `not a valid .docx (zip) file: ${(e as Error).message}`);
    }
    const pkg = new Package();
    for (const [name, data] of Object.entries(entries)) {
      // Rust skips `!file.is_file()` — directory entries end in "/".
      if (name.endsWith("/")) continue;
      pkg.parts.push({ name, data });
    }
    return pkg;
  }

  get(name: string): Uint8Array | undefined {
    return this.parts.find((p) => p.name === name)?.data;
  }

  getStr(name: string): string | undefined {
    const bytes = this.get(name);
    if (bytes === undefined) return undefined;
    try {
      return textDecoder.decode(bytes);
    } catch {
      throw new DocxError("malformed", `${name} is not valid UTF-8`);
    }
  }

  /** Replace an existing part's bytes, or append it if new (new parts go last). */
  set(name: string, data: Uint8Array): void {
    const part = this.parts.find((p) => p.name === name);
    if (part) {
      part.data = data;
    } else {
      this.parts.push({ name, data });
    }
  }

  write(): Uint8Array {
    const files: Record<string, [Uint8Array, { mtime: Date }]> = {};
    for (const part of this.parts) {
      // Fixed DOS-epoch timestamp, matching zip 0.6's default, for
      // deterministic output.
      files[part.name] = [part.data, { mtime: new Date(1980, 0, 1, 0, 0, 0) }];
    }
    try {
      return zipSync(files, { level: 9 });
    } catch (e) {
      throw new DocxError("zip", `${(e as Error).message}`);
    }
  }
}

export function strToBytes(s: string): Uint8Array {
  return textEncoder.encode(s);
}
