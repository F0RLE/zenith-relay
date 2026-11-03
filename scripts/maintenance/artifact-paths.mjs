import { lstatSync } from "node:fs";
import { isAbsolute, join, relative, resolve, sep } from "node:path";

/** Resolve only Relay artifacts; the caller's working directory is irrelevant. */
export function artifactPathsToClean(repositoryRoot, includeAllArtifacts = false) {
  const desktopFrontendRoot = join(repositoryRoot, "src");
  const webPreviewRoot = join(repositoryRoot, "web");
  const frontendBuildPaths = [
    join(desktopFrontendRoot, ".build"),
    join(desktopFrontendRoot, "dist"),
    join(webPreviewRoot, "dist"),
  ];
  if (!includeAllArtifacts) return frontendBuildPaths;
  return [
    ...frontendBuildPaths,
    join(desktopFrontendRoot, "test-results"),
    join(desktopFrontendRoot, "playwright-report"),
    join(desktopFrontendRoot, "output", "playwright"),
    join(webPreviewRoot, "test-results"),
    join(webPreviewRoot, "playwright-report"),
    join(repositoryRoot, "src-tauri", "target"),
    join(repositoryRoot, "src-tauri", "gen", "schemas"),
    join(repositoryRoot, "crates", "relay-core", "target"),
    join(repositoryRoot, "target"),
  ];
}

export function assertSafeCleanupPaths(repositoryRoot, artifactPaths, inspectPath = lstatSync) {
  const resolvedRoot = resolve(repositoryRoot);
  for (const artifactPath of artifactPaths) {
    const relativePath = relative(resolvedRoot, resolve(artifactPath));
    if (!relativePath || isAbsolute(relativePath) || relativePath.split(sep)[0] === "..") {
      throw new Error(`Refusing cleanup outside the repository: ${artifactPath}`);
    }
    let currentPath = resolvedRoot;
    for (const segment of ["", ...relativePath.split(sep)]) {
      currentPath = join(currentPath, segment);
      try {
        const metadata = inspectPath(currentPath);
        if (metadata.isSymbolicLink() || !metadata.isDirectory()) {
          throw new Error(`Refusing cleanup through a link or non-directory: ${currentPath}`);
        }
      } catch (error) {
        if (error.code !== "ENOENT") throw error;
      }
    }
  }
}
