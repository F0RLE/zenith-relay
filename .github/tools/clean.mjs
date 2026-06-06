import { rmSync } from "node:fs";

const mode = process.argv.includes("--all") ? "all" : "default";
const paths = mode === "all"
  ? [".build", "dist", "../src-tauri/target", "../src-tauri/gen", "../target", "../gen"]
  : [".build", "dist"];

for (const path of paths) {
  rmSync(path, { recursive: true, force: true });
}

console.log(`Cleaned ${mode === "all" ? "all local" : "frontend"} build artifacts.`);
