import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
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
