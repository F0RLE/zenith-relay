import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { minify } from "terser";

/**
 * Keep the pre-bundle watchdog independent without shipping source whitespace.
 * @returns {import("vite").Plugin}
 */
export function minifyStartup() {
  let startupPath = "";
  return {
    name: "relay-minify-startup",
    apply: "build",
    configResolved(config) {
      startupPath = resolve(config.publicDir, "startup.js");
    },
    async generateBundle() {
      this.addWatchFile(startupPath);
      const { code } = await minify(await readFile(startupPath, "utf8"), {
        ecma: 5,
        compress: { passes: 2 },
      });
      if (!code) this.error("Startup watchdog minification produced no code");
      this.emitFile({ type: "asset", fileName: "startup.js", source: code });
    },
  };
}
