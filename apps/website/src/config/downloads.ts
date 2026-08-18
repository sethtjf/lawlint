export const DOWNLOAD_BASE_URL =
  import.meta.env.PUBLIC_DOWNLOAD_BASE_URL || "https://assets.lawlint.com/downloads";

// Production deploys provide PUBLIC_DOWNLOAD_VERSION from the last published
// release. Local and PR builds intentionally use the mutable latest prefix:
// their branch version may be a release-please bump whose artifacts do not
// exist yet.
export const DOWNLOAD_VERSION = import.meta.env.PUBLIC_DOWNLOAD_VERSION;

if (DOWNLOAD_VERSION && !/^(?!.*\.\.)[A-Za-z0-9][A-Za-z0-9._+-]*$/.test(DOWNLOAD_VERSION)) {
  throw new Error("The published lawlint download version is invalid.");
}

const downloadPrefix = DOWNLOAD_VERSION ? `releases/v${DOWNLOAD_VERSION}` : "latest";
export const RELEASE_DOWNLOAD_PATH = `${DOWNLOAD_BASE_URL.replace(/\/$/, "")}/${downloadPrefix}`;

export const downloadUrl = (asset: string) => `${RELEASE_DOWNLOAD_PATH}/${asset}`;
