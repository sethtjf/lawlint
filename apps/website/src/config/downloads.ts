import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

export const DOWNLOAD_BASE_URL =
  import.meta.env.PUBLIC_DOWNLOAD_BASE_URL || "https://assets.lawlint.com/downloads";

// Astro evaluates this module from its generated build directory, so resolve
// Cargo.toml from the working directory instead of relying on import.meta.url.
const cargoTomlPath = [
  resolve(process.cwd(), "Cargo.toml"),
  resolve(process.cwd(), "../../Cargo.toml"),
].find((path) => existsSync(path));
const cargoVersion = cargoTomlPath
  ? readFileSync(cargoTomlPath, "utf8").match(/^version\s*=\s*"([^"]+)"/m)?.[1]
  : undefined;
export const DOWNLOAD_VERSION = import.meta.env.PUBLIC_DOWNLOAD_VERSION || cargoVersion;

if (!DOWNLOAD_VERSION || !/^[A-Za-z0-9][A-Za-z0-9._+-]*$/.test(DOWNLOAD_VERSION)) {
  throw new Error("Could not determine a valid lawlint download version.");
}

export const RELEASE_DOWNLOAD_PATH = `${DOWNLOAD_BASE_URL.replace(/\/$/, "")}/releases/v${DOWNLOAD_VERSION}`;

export const downloadUrl = (asset: string) => `${RELEASE_DOWNLOAD_PATH}/${asset}`;
