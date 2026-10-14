// Read/write the OPC (zip) container while preserving every part. Untouched
// parts round-trip content-identical — the whole point of not going through
// a lossy typed model.

import { Zip, ZipDeflate, unzipSync } from "fflate";
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
    // A plain-object result would reorder integer-like names ("0") to the
    // front and collapse duplicate names; the filter callback fires once per
    // central-directory entry, in order, so collect the names there.
    const names: string[] = [];
    let entries: Record<string, Uint8Array>;
    try {
      entries = unzipSync(bytes, {
        filter: (file) => {
          names.push(file.name);
          return true;
        },
      });
    } catch (e) {
      throw new DocxError("zip", `not a valid .docx (zip) file: ${(e as Error).message}`);
    }
    const pkg = new Package();
    for (const name of names) {
      // Rust skips `!file.is_file()` — directory entries end in "/".
      if (name.endsWith("/")) continue;
      const data = entries[name];
      if (data !== undefined) pkg.parts.push({ name, data });
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
    // zipSync takes a record, which would reorder integer-like names and
    // collapse duplicates; the streaming Zip writes entries in push order.
    const chunks: Uint8Array[] = [];
    try {
      const zip = new Zip((_err, chunk) => {
        chunks.push(chunk);
      });
      for (const part of this.parts) {
        // Fixed DOS-epoch timestamp, matching zip 0.6's default, for
        // deterministic output; level 6 is Rust's deflate default.
        const file = new ZipDeflate(part.name, { level: 6 });
        file.mtime = new Date(1980, 0, 1, 0, 0, 0);
        zip.add(file);
        file.push(part.data, true);
      }
      zip.end();
    } catch (e) {
      throw new DocxError("zip", `${(e as Error).message}`);
    }
    const total = chunks.reduce((n, c) => n + c.length, 0);
    const out = new Uint8Array(total);
    let off = 0;
    for (const c of chunks) {
      out.set(c, off);
      off += c.length;
    }
    return out;
  }
}

export function strToBytes(s: string): Uint8Array {
  return textEncoder.encode(s);
}
