import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

export const DOWNLOAD_BASE_URL =
  import.meta.env.PUBLIC_DOWNLOAD_BASE_URL || "https://assets.lawlint.com/downloads";

// Production deploys provide PUBLIC_DOWNLOAD_VERSION from the last published
// release. Local/PR builds fall back to Cargo.toml; resolve it from the working
// directory because Astro evaluates this module from its generated build dir.
const cargoTomlPath = [
  resolve(process.cwd(), "Cargo.toml"),
  resolve(process.cwd(), "../../Cargo.toml"),
].find((path) => existsSync(path));
const cargoVersion = cargoTomlPath
  ? readFileSync(cargoTomlPath, "utf8").match(/^version\s*=\s*"([^"]+)"/m)?.[1]
  : undefined;
export const DOWNLOAD_VERSION = import.meta.env.PUBLIC_DOWNLOAD_VERSION || cargoVersion;

if (!DOWNLOAD_VERSION || !/^(?!.*\.\.)[A-Za-z0-9][A-Za-z0-9._+-]*$/.test(DOWNLOAD_VERSION)) {
  throw new Error("Could not determine a valid lawlint download version.");
}

export const RELEASE_DOWNLOAD_PATH = `${DOWNLOAD_BASE_URL.replace(/\/$/, "")}/releases/v${DOWNLOAD_VERSION}`;

export const downloadUrl = (asset: string) => `${RELEASE_DOWNLOAD_PATH}/${asset}`;
