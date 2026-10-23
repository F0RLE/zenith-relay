import { expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { fileURLToPath } from "node:url";
import { prependPath, usesDefaultWindowsReleaseOutput } from "./tauri-env.mjs";

test("only default Windows release output can receive the local production marker", () => {
  for (const args of [
    [], ["--no-bundle"], ["--verbose", "--ci"], ["-vv"],
    ["-vfmodel"], ["-f", "model"], ["--features", "model"], ["-bnsis"],
  ]) {
    expect(usesDefaultWindowsReleaseOutput(args, "win32", {})).toBe(true);
    expect(usesDefaultWindowsReleaseOutput(args, "linux", {})).toBe(false);
    expect(usesDefaultWindowsReleaseOutput(args, "darwin", {})).toBe(false);
  }
});

test.each([
  ["--help"], ["-h"], ["--version"], ["-V"],
  ["--debug"], ["-d"], ["-vd"],
  ["--target", "x86_64-pc-windows-msvc"], ["--target=x86_64-pc-windows-msvc"],
  ["-t", "x86_64-pc-windows-msvc"], ["-tx86_64-pc-windows-msvc"], ["-vtx86_64-pc-windows-msvc"],
  ["--config", "custom.json"], ["--config=custom.json"], ["-ccustom.json"],
  ["--runner", "custom-runner"], ["--runner=custom-runner"], ["-rcustom-runner"],
  ["--", "--profile", "dev"], ["--", "--target-dir", "custom-target"],
].map((args) => [args]))("alternate build arguments %j cannot mark a stale default executable", (args) => {
  expect(usesDefaultWindowsReleaseOutput(args, "win32", {})).toBe(false);
});

test("Cargo target environment overrides cannot mark the default executable", () => {
  for (const name of ["CARGO_TARGET_DIR", "CARGO_BUILD_TARGET", "CARGO_BUILD_TARGET_DIR"]) {
    expect(usesDefaultWindowsReleaseOutput([], "win32", { [name]: "alternate" })).toBe(false);
  }
});

test("tool paths precede the inherited PATH without duplicates or missing directories", () => {
  const root = mkdtempSync(join(tmpdir(), "relay-path-test-"));
  try {
    const tool = join(root, "tool bin");
    const inherited = join(root, "inherited bin");
    mkdirSync(tool);
    mkdirSync(inherited);
    const env = { PATH: [inherited, tool, inherited].join(delimiter) };
    prependPath(env, [null, "", join(root, "missing"), tool, tool]);
    expect(env.PATH.split(delimiter)).toEqual([tool, inherited]);
    prependPath(env, [tool]);
    expect(env.PATH.split(delimiter)).toEqual([tool, inherited]);
    if (process.platform === "win32") {
      expect(env.Path).toBe(env.PATH);
      env.Path = [inherited, tool.toUpperCase()].join(delimiter);
      prependPath(env, [tool]);
      expect(env.Path.split(delimiter)).toEqual([tool, inherited]);
      expect(env.PATH).toBe(env.Path);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("setup and start help works outside the repository without starting the toolchain", () => {
  for (const name of ["setup-dev.mjs", "start-dev.mjs"]) {
    const script = fileURLToPath(new URL(`../setup/${name}`, import.meta.url));
    const result = spawnSync(process.execPath, [script, "--help"], {
      cwd: tmpdir(), encoding: "utf8", windowsHide: true,
    });
    expect(result.status, result.stderr).toBe(0);
    expect(result.stdout).toContain("Usage:");
    expect(result.stdout).not.toContain("[setup]");
    expect(result.stdout).not.toContain("[dev]");
  }
});
