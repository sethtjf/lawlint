/// <reference types="astro/client" />

interface ImportMetaEnv {
  readonly PUBLIC_DOWNLOAD_BASE_URL?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
