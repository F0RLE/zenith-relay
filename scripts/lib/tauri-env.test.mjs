import { expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { fileURLToPath } from "node:url";
import { configureNinjaEnvironment, prependPath, repoRoot, usesDefaultWindowsReleaseOutput } from "./tauri-env.mjs";

test("Ninja does not inherit Visual Studio generator arguments", () => {
  const env = {
    CMAKE_GENERATOR: "Visual Studio 17 2022", CMAKE_GENERATOR_PLATFORM: "x64",
    CMAKE_GENERATOR_TOOLSET: "v143", CMAKE_GENERATOR_INSTANCE: "old-vs", KEEP: "unchanged",
  };
  configureNinjaEnvironment(env, "portable/ninja.exe");
  expect(env).toEqual({ CMAKE_GENERATOR: "Ninja", CMAKE_MAKE_PROGRAM: "portable/ninja.exe", KEEP: "unchanged" });
});

test.skipIf(process.platform !== "win32")("Rust selection follows Relay's toolchain from outside the repository", () => {
  const expected = spawnSync("rustc", ["--print", "sysroot"], {
    cwd: repoRoot(), encoding: "utf8", shell: true, windowsHide: true,
  });
  expect(expected.status, expected.stderr).toBe(0);
  const module = new URL("./tauri-env.mjs", import.meta.url).href;
  const code = `import {withZenithRustEnv} from ${JSON.stringify(module)};
    import {spawnSync} from 'node:child_process';
    const r=spawnSync('rustc',['--print','sysroot'],{env:withZenithRustEnv(),encoding:'utf8',shell:true,windowsHide:true});
    process.stdout.write(r.stdout ?? ''); process.stderr.write(r.stderr ?? ''); process.exit(r.status ?? 1);`;
  const actual = spawnSync(process.execPath, ["-e", code], { cwd: tmpdir(), encoding: "utf8", windowsHide: true });
  expect(actual.status, actual.stderr).toBe(0);
  expect(actual.stdout.trim()).toBe(expected.stdout.trim());
});

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
