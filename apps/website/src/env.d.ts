/// <reference types="astro/client" />

interface ImportMetaEnv {
  readonly PUBLIC_DOWNLOAD_BASE_URL?: string;
  readonly PUBLIC_DOWNLOAD_VERSION?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
