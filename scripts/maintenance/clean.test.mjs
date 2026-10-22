import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { artifactPathsToClean, assertSafeCleanupPaths } from "./artifact-paths.mjs";

describe("build artifact cleanup scope", () => {
  const root = resolve("synthetic-workspace", "zenith-relay");

  test("default cleanup targets Desktop and web builds regardless of invocation directory", () => {
    expect(artifactPathsToClean(root)).toEqual([
      join(root, "src", ".build"), join(root, "src", "dist"), join(root, "web", "dist"),
    ]);
  });

  test("full cleanup never resolves a path outside this repository", () => {
    const paths = artifactPathsToClean(root, true);
    expect(paths).toHaveLength(12);
    expect(new Set(paths).size).toBe(paths.length);
    for (const path of paths) {
      const withinRepository = relative(root, path);
      expect(isAbsolute(path)).toBe(true);
      expect(isAbsolute(withinRepository)).toBe(false);
      expect(withinRepository.startsWith("..")).toBe(false);
      expect(withinRepository).not.toBe("");
    }
    expect(paths).toContain(join(root, "crates", "relay-core", "target"));
    expect(paths).toContain(join(root, "src-tauri", "target"));
    expect(paths).toContain(join(root, "web", "test-results"));
    expect(paths).toContain(join(root, "web", "playwright-report"));
  });

  test("help and unknown options exit before cleanup", () => {
    const script = fileURLToPath(new URL("./clean.mjs", import.meta.url));
    for (const [option, status] of [["--help", 0], ["--not-a-cleanup-option", 2]]) {
      const result = spawnSync(process.execPath, [script, option], { encoding: "utf8", windowsHide: true });
      expect(result.status).toBe(status);
    }
  });

  test("full cleanup includes browser captures without removing all output or documentation images", () => {
    const paths = artifactPathsToClean(root, true);
    expect(paths).toContain(join(root, "src", "output", "playwright"));
    expect(paths).not.toContain(join(root, "src", "output"));
    expect(paths).not.toContain(join(root, "docs", "screenshots"));
  });

  test("full cleanup removes permission schemas without deleting other gen contents", () => {
    const paths = artifactPathsToClean(root, true);
    expect(paths).toContain(join(root, "src-tauri", "gen", "schemas"));
    expect(paths).not.toContain(join(root, "src-tauri", "gen"));
    expect(paths).not.toContain(join(root, "gen"));
  });
});

describe("cleanup path safety", () => {
  const repositoryRoot = resolve("synthetic-workspace", "zenith-relay");
  const directoryMetadata = { isSymbolicLink: () => false, isDirectory: () => true };

  test("inspects every target and ancestor without deleting anything", () => {
    const inspectedPaths = [];
    const artifactPaths = artifactPathsToClean(repositoryRoot, true);
    assertSafeCleanupPaths(repositoryRoot, artifactPaths, (path) => {
      inspectedPaths.push(path);
      return directoryMetadata;
    });
    for (const path of artifactPaths) expect(inspectedPaths).toContain(path);
    expect(inspectedPaths).toContain(repositoryRoot);
    expect(inspectedPaths).toContain(join(repositoryRoot, "src", "output"));
  });

  test.each(["", "web", join("web", "dist")])("rejects a linked root, parent or target: %s", (linkedRelativePath) => {
    const linkedPath = join(repositoryRoot, linkedRelativePath);
    expect(() => assertSafeCleanupPaths(repositoryRoot, [join(repositoryRoot, "web", "dist")], (path) => ({
      ...directoryMetadata,
      isSymbolicLink: () => path === linkedPath,
    }))).toThrow("Refusing cleanup through a link or non-directory");
  });

  test("a missing target does not hide a linked parent", () => {
    const artifactPath = join(repositoryRoot, "web", "dist");
    expect(() => assertSafeCleanupPaths(repositoryRoot, [artifactPath], (path) => {
      if (path === artifactPath) throw Object.assign(new Error("missing"), { code: "ENOENT" });
      return { ...directoryMetadata, isSymbolicLink: () => path === join(repositoryRoot, "web") };
    })).toThrow("Refusing cleanup through a link or non-directory");
  });

  test("missing directories remain safe to skip", () => {
    expect(() => assertSafeCleanupPaths(repositoryRoot, artifactPathsToClean(repositoryRoot, true), () => {
      throw Object.assign(new Error("missing"), { code: "ENOENT" });
    })).not.toThrow();
  });

  test("metadata errors abort cleanup rather than treating the path as absent", () => {
    expect(() => assertSafeCleanupPaths(repositoryRoot, artifactPathsToClean(repositoryRoot), () => {
      throw Object.assign(new Error("metadata unavailable"), { code: "EACCES" });
    })).toThrow("metadata unavailable");
  });

  test("a file at an artifact path is not deleted as a directory", () => {
    const artifactPath = join(repositoryRoot, "src", "dist");
    expect(() => assertSafeCleanupPaths(repositoryRoot, [artifactPath], (path) => ({
      ...directoryMetadata,
      isDirectory: () => path !== artifactPath,
    }))).toThrow("Refusing cleanup through a link or non-directory");
  });

  test.each([repositoryRoot, resolve(repositoryRoot, ".."), resolve(repositoryRoot, "..", "other", "dist")])("rejects the repository itself or an outside target: %s", (path) => {
    expect(() => assertSafeCleanupPaths(repositoryRoot, [path], () => directoryMetadata))
      .toThrow("Refusing cleanup outside the repository");
  });
});
