import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";

const metadata = JSON.parse(
  execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
    encoding: "utf8",
  }),
);

const workspaceMembers = new Set(metadata.workspace_members);
const packages = metadata.packages
  .filter((pkg) => workspaceMembers.has(pkg.id))
  .map((pkg) => ({ name: pkg.name, version: pkg.version }));

if (packages.length === 0) {
  throw new Error("Cargo metadata did not contain any workspace packages.");
}

const versions = new Map(packages.map((pkg) => [pkg.name, pkg.version]));
const lockPath = "Cargo.lock";
const lock = readFileSync(lockPath, "utf8");
const blocks = lock.split(/(?=^\[\[package\]\]\s*$)/mu);
const seen = new Set();
let updated = lock;

for (const block of blocks) {
  const name = block.match(/^name = "([^"]+)"$/mu)?.[1];
  if (!name || !versions.has(name)) continue;

  const expected = versions.get(name);
  const versionPattern = /^version = "[^"]+"$/mu;
  if (!versionPattern.test(block)) {
    throw new Error(`Cargo.lock package ${name} has no version field.`);
  }

  const nextBlock = block.replace(versionPattern, `version = "${expected}"`);
  updated = updated.replace(block, nextBlock);
  seen.add(name);
}

const missing = packages.map((pkg) => pkg.name).filter((name) => !seen.has(name));
if (missing.length > 0) {
  throw new Error(`Cargo.lock is missing workspace packages: ${missing.join(", ")}`);
}

if (updated === lock) {
  console.log("Cargo.lock workspace versions are already synchronized.");
} else {
  writeFileSync(lockPath, updated);
  console.log(`Synchronized ${packages.length} workspace package versions in ${lockPath}.`);
}
