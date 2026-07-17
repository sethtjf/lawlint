export const DOWNLOAD_BASE_URL =
  import.meta.env.PUBLIC_DOWNLOAD_BASE_URL || "https://assets.lawlint.com/downloads";

export const LATEST_DOWNLOAD_PATH = `${DOWNLOAD_BASE_URL.replace(/\/$/, "")}/latest`;

export const downloadUrl = (asset: string) => `${LATEST_DOWNLOAD_PATH}/${asset}`;
