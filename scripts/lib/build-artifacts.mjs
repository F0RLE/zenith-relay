import { join } from "node:path";

/** Resolve only Relay artifacts; the caller's working directory is irrelevant. */
export function artifactPathsToClean(repositoryRoot, includeRustArtifacts = false) {
  const frontendRoot = join(repositoryRoot, "src");
  const frontendPaths = [join(frontendRoot, ".build"), join(frontendRoot, "dist")];
  if (!includeRustArtifacts) return frontendPaths;
  return [
    ...frontendPaths,
    join(frontendRoot, "test-results"),
    join(frontendRoot, "playwright-report"),
    join(repositoryRoot, "src-tauri", "target"),
    join(repositoryRoot, "src-tauri", "gen"),
    join(repositoryRoot, "crates", "relay-core", "target"),
    join(repositoryRoot, "relay-server", "target"),
    join(repositoryRoot, "target"),
    join(repositoryRoot, "gen"),
  ];
}
