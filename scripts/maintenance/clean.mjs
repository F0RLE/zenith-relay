import { rmSync } from "node:fs";

const cleanMode = process.argv.includes("--all") ? "all" : "default";
const artifactPaths = cleanMode === "all"
  ? [
      ".build",
      "dist",
      "test-results",
      "playwright-report",
      "../src-tauri/target",
      "../src-tauri/gen",
      "../crates/relay-core/target",
      "../relay-server/target",
      "../target",
      "../gen",
    ]
  : [".build", "dist"];

for (const artifactPath of artifactPaths) {
  rmSync(artifactPath, { recursive: true, force: true });
}

console.log(`Cleaned ${cleanMode === "all" ? "all local" : "frontend"} build artifacts.`);
