import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { startupChunks } from "./build/startupChunks.ts";

export default defineConfig({
  plugins: [react(), startupChunks()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    manifest: true,
    target: "es2020",
    outDir: "dist",
    emptyOutDir: true,
    rolldownOptions: {
      output: {
        // Icons share one side-effect-free module instead of many tiny
        // chunks. Keep pages and language resources independently lazy.
        codeSplitting: {
          groups: [{ name: "icons", test: /node_modules[\\/]lucide-react[\\/]/ }],
        },
      },
    },
  },
});
