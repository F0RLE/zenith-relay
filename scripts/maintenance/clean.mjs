import { rmSync } from "node:fs";
import { artifactPathsToClean, assertSafeCleanupPaths, incrementalArtifactPaths } from "./artifact-paths.mjs";
import { repoRoot } from "../lib/tauri-env.mjs";

const cliArgs = process.argv.slice(2);
if (cliArgs.some((argument) => argument === "--help" || argument === "-h")) {
  console.log("Usage: bun scripts/maintenance/clean.mjs [--all | --incremental]");
  console.log("Remove frontend builds; --all also removes Rust builds and generated test artifacts.");
  console.log("--incremental removes only Rust incremental caches, keeping dependencies and release builds.");
  process.exit(0);
}
if (cliArgs.some((argument) => !["--all", "--incremental"].includes(argument))) {
  console.error("Unknown cleanup option. Use --help for usage.");
  process.exit(2);
}
const includeAllArtifacts = cliArgs.includes("--all");
const incrementalOnly = cliArgs.includes("--incremental");
if (includeAllArtifacts && incrementalOnly) {
  console.error("Choose either --all or --incremental.");
  process.exit(2);
}
const repositoryRoot = repoRoot();
const artifactPaths = incrementalOnly
  ? incrementalArtifactPaths(repositoryRoot)
  : artifactPathsToClean(repositoryRoot, includeAllArtifacts);
assertSafeCleanupPaths(repositoryRoot, artifactPaths);
for (const artifactPath of artifactPaths) {
  rmSync(artifactPath, { recursive: true, force: true });
}

console.log(incrementalOnly
  ? "Cleaned Rust incremental caches. The next debug build may take longer."
  : `Cleaned ${includeAllArtifacts ? "all local" : "frontend"} build artifacts.`);
