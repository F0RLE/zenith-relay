import { rmSync } from "node:fs";
import { artifactPathsToClean } from "../lib/build-artifacts.mjs";
import { repoRoot } from "../lib/tauri-env.mjs";

const cliArgs = process.argv.slice(2);
if (cliArgs.some((argument) => argument === "--help" || argument === "-h")) {
  console.log("Usage: bun scripts/maintenance/clean.mjs [--all]");
  console.log("Remove Relay frontend artifacts; --all also removes local Rust build artifacts.");
  process.exit(0);
}
if (cliArgs.some((argument) => argument !== "--all")) {
  console.error("Unknown cleanup option. Use --help for usage.");
  process.exit(2);
}
const includeRustArtifacts = cliArgs.includes("--all");
for (const artifactPath of artifactPathsToClean(repoRoot(), includeRustArtifacts)) {
  rmSync(artifactPath, { recursive: true, force: true });
}

console.log(`Cleaned ${includeRustArtifacts ? "all local" : "frontend"} build artifacts.`);
