import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const rootPkg = JSON.parse(
  readFileSync(resolve(__dirname, "../package.json"), "utf-8")
) as { version: string };

export default defineConfig({
  plugins: [react()],
  base: "/iKoch/",
  define: {
    __IKOCH_VERSION__: JSON.stringify(rootPkg.version),
  },
});
