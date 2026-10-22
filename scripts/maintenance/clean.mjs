import { rmSync } from "node:fs";
import { artifactPathsToClean, assertSafeCleanupPaths } from "./artifact-paths.mjs";
import { repoRoot } from "../lib/tauri-env.mjs";

const cliArgs = process.argv.slice(2);
if (cliArgs.some((argument) => argument === "--help" || argument === "-h")) {
  console.log("Usage: bun scripts/maintenance/clean.mjs [--all]");
  console.log("Remove frontend builds; --all also removes Rust builds and generated test artifacts.");
  process.exit(0);
}
if (cliArgs.some((argument) => argument !== "--all")) {
  console.error("Unknown cleanup option. Use --help for usage.");
  process.exit(2);
}
const includeAllArtifacts = cliArgs.includes("--all");
const repositoryRoot = repoRoot();
const artifactPaths = artifactPathsToClean(repositoryRoot, includeAllArtifacts);
assertSafeCleanupPaths(repositoryRoot, artifactPaths);
for (const artifactPath of artifactPaths) {
  rmSync(artifactPath, { recursive: true, force: true });
}

console.log(`Cleaned ${includeAllArtifacts ? "all local" : "frontend"} build artifacts.`);
