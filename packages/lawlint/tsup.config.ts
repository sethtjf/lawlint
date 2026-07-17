import { defineConfig } from "tsup";

export default defineConfig({
  entry: ["src/index.ts", "src/cli.ts"],
  tsconfig: "tsconfig.build.json",
  format: ["esm", "cjs"],
  dts: { entry: "src/index.ts" },
  clean: true,
  sourcemap: true,
  splitting: false,
});
