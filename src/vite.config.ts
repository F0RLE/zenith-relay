import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { startupChunks } from "./build/startupChunks.ts";
import { minifyStartup } from "./build/minifyStartup.mjs";
import { stableOutput } from "./build/stableOutput.mjs";

export default defineConfig({
  plugins: [react(), startupChunks(), minifyStartup(), stableOutput()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    manifest: true,
    target: "es2020",
    // Spend a little more build time to reduce shipped JavaScript without
    // changing lazy-entry boundaries or the supported WebView target.
    minify: "terser",
    terserOptions: { compress: { passes: 2 } },
    // The following bundle gate reports gzip sizes once, using its exact limits.
    reportCompressedSize: false,
    outDir: "dist",
    emptyOutDir: true,
    rolldownOptions: {
      output: {
        // Native output optimization complements Terser; neither the WebView
        // target nor lazy page/language boundaries change.
        minify: true,
        // Icons share one side-effect-free module instead of many tiny
        // chunks. Keep pages and language resources independently lazy.
        codeSplitting: {
          groups: [
            { name: "react-runtime", test: /node_modules[\\/](?:react|react-dom|scheduler)[\\/]/, priority: 40 },
            { name: "icons", test: /node_modules[\\/]lucide-react[\\/]/ },
          ],
        },
      },
    },
  },
});
