import { defineConfig } from "tsup";

export default defineConfig({
  entry: ["src/index.ts", "src/cli.ts", "src/browser.ts"],
  tsconfig: "tsconfig.build.json",
  format: ["esm", "cjs"],
  dts: { entry: ["src/index.ts", "src/browser.ts"] },
  clean: true,
  sourcemap: true,
  splitting: false,
});
