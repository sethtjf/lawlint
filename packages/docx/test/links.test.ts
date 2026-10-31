// Ported from crates/lawlint-docx/src/links.rs #[cfg(test)].

import { expect, test } from "bun:test";
import { find, strip, stripAiTracking } from "../src/links";
import { Package, strToBytes } from "../src/pkg";

test("strips only AI utm_source", () => {
  const cases: [string, string][] = [
    ["https://a.test/x?utm_source=chatgpt.com", "https://a.test/x"],
    ["https://a.test/x?utm_source=chatgpt.com&page=2", "https://a.test/x?page=2"],
    ["https://a.test/x?page=2&utm_source=chatgpt.com#s3", "https://a.test/x?page=2#s3"],
    ["https://a.test/x?UTM_SOURCE=OpenAI", "https://a.test/x"],
    ["https://a.test/x?utm_source=openai.com&utm_medium=x", "https://a.test/x?utm_medium=x"],
  ];
  for (const [input, want] of cases) expect(stripAiTracking(input)).toBe(want);
  for (const clean of [
    "https://a.test/x",
    "https://a.test/x?utm_source=newsletter",
    "https://a.test/x?utm_source=chatgpt.com.example",
    "https://a.test/x#utm_source=chatgpt.com",
  ]) {
    expect(stripAiTracking(clean)).toBeUndefined();
  }
});

const RELS = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://a.test/case?x=1&amp;utm_source=chatgpt.com" TargetMode="External"/><Relationship Id="rId6" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://a.test/clean" TargetMode="External"/></Relationships>`;

function pkgWith(rels: string): Package {
  const pkg = new Package();
  pkg.set("word/_rels/document.xml.rels", strToBytes(rels));
  pkg.set("word/document.xml", strToBytes("<w:document/>"));
  return pkg;
}

test("finds and strips hyperlink targets only", () => {
  const pkg = pkgWith(RELS);
  const found = find(pkg);
  expect(found.length).toBe(1);
  expect(found[0]?.url).toBe("https://a.test/case?x=1&utm_source=chatgpt.com");
  expect(found[0]?.cleaned).toBe("https://a.test/case?x=1");

  expect(strip(pkg)).toEqual(found);
  expect(pkg.getStr("word/_rels/document.xml.rels")).toBe(
    RELS.replace("?x=1&amp;utm_source=chatgpt.com", "?x=1"),
  );
  expect(find(pkg)).toEqual([]);
});

test("untouched parts stay byte-identical", () => {
  const rels = RELS.replace("&amp;utm_source=chatgpt.com", "");
  const pkg = pkgWith(rels);
  expect(strip(pkg)).toEqual([]);
  expect(pkg.get("word/_rels/document.xml.rels")).toEqual(strToBytes(rels));
});

test("single-quoted attributes and internal targets", () => {
  const rels = `<Relationships><Relationship Target='https://a.test/?utm_source=chatgpt.com' TargetMode='External' Id='r1' Type='http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink'/><Relationship Id="r2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://a.test/?utm_source=chatgpt.com"/></Relationships>`;
  const changed = strip(pkgWith(rels));
  // Only the External one: an internal target is a package part, not a URL.
  expect(changed.length).toBe(1);
  expect(changed[0]?.cleaned).toBe("https://a.test/");
});
