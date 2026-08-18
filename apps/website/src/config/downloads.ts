import { readFileSync } from "node:fs";

export const DOWNLOAD_BASE_URL =
  import.meta.env.PUBLIC_DOWNLOAD_BASE_URL || "https://assets.lawlint.com/downloads";

const cargoVersion = readFileSync(new URL("../../../../Cargo.toml", import.meta.url), "utf8").match(
  /^version\s*=\s*"([^"]+)"/m,
)?.[1];
export const DOWNLOAD_VERSION = import.meta.env.PUBLIC_DOWNLOAD_VERSION || cargoVersion;

if (!DOWNLOAD_VERSION || !/^[A-Za-z0-9][A-Za-z0-9._+-]*$/.test(DOWNLOAD_VERSION)) {
  throw new Error("Could not determine a valid lawlint download version.");
}

export const RELEASE_DOWNLOAD_PATH = `${DOWNLOAD_BASE_URL.replace(/\/$/, "")}/releases/v${DOWNLOAD_VERSION}`;

export const downloadUrl = (asset: string) => `${RELEASE_DOWNLOAD_PATH}/${asset}`;
