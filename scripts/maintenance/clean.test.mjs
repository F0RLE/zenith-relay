import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { artifactPathsToClean } from "../lib/build-artifacts.mjs";

describe("build artifact cleanup scope", () => {
  const root = resolve("synthetic-workspace", "zenith-relay");

  test("default cleanup targets the frontend regardless of invocation directory", () => {
    expect(artifactPathsToClean(root)).toEqual([
      join(root, "src", ".build"), join(root, "src", "dist"),
    ]);
  });

  test("full cleanup never resolves a path outside this repository", () => {
    const paths = artifactPathsToClean(root, true);
    expect(paths).toHaveLength(10);
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
  });

  test("help and unknown options exit before cleanup", () => {
    const script = fileURLToPath(new URL("./clean.mjs", import.meta.url));
    for (const [option, status] of [["--help", 0], ["--not-a-cleanup-option", 2]]) {
      const result = spawnSync(process.execPath, [script, option], { encoding: "utf8", windowsHide: true });
      expect(result.status).toBe(status);
    }
  });
});
