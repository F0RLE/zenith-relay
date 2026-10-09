import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { delimiter, join, resolve } from "node:path";
import { repoRoot, withZenithRustEnv } from "../lib/tauri-env.mjs";

const root = repoRoot();
const frontendRoot = join(root, "src");
const wantsBrowsers = process.argv.includes("--with-browsers");
const helpRequested = process.argv.includes("--help") || process.argv.includes("-h");

if (helpRequested) {
  console.log("Usage: bun run setup [--with-browsers]");
  console.log("Install locked frontend and Rust dependencies for local development.");
  console.log("Add --with-browsers when Playwright Chromium is needed for e2e tests.");
  process.exit(0);
}

if (!process.versions.bun) {
  console.error("Run this setup with Bun: bun run setup");
  process.exit(1);
}

function prependPath(env, entries) {
  const pathKey = process.platform === "win32" ? "Path" : "PATH";
  const existingPathValue = env[pathKey] ?? env.PATH ?? "";
  const availableEntries = entries.filter((entry) => entry && existsSync(entry));
  env[pathKey] = [...availableEntries, existingPathValue].filter(Boolean).join(delimiter);
  env.PATH = env[pathKey];
}

function setupEnvironment() {
  const env = { ...process.env };
  if (process.platform === "win32") {
    env.DEVELOPMENT_HOME ??= env.USERPROFILE
      ? join(env.USERPROFILE, "Development")
      : undefined;
  }
  if (process.platform === "win32" && env.DEVELOPMENT_HOME) {
    env.CARGO_HOME ??= join(env.DEVELOPMENT_HOME, "rust", "cargo-home");
    env.RUSTUP_HOME ??= join(env.DEVELOPMENT_HOME, "rust", "rustup-home");
    env.BUN_INSTALL ??= join(env.DEVELOPMENT_HOME, "bun");
    prependPath(env, [
      join(env.CARGO_HOME, "bin"),
      join(env.BUN_INSTALL, "bin"),
    ]);
  }
  return withZenithRustEnv(env);
}

function commandAvailable(command, args, env) {
  const commandResult = spawnSync(command, args, {
    cwd: root,
    env,
    stdio: "ignore",
    windowsHide: true,
  });
  return !commandResult.error && commandResult.status === 0;
}

function windowsToolchainReady(env) {
  return [
    ["where.exe", ["cl.exe"]],
    ["where.exe", ["link.exe"]],
    ["where.exe", ["rc.exe"]],
    ["where.exe", ["cmake.exe"]],
    ["where.exe", ["ninja.exe"]],
  ].every(([command, args]) => commandAvailable(command, args, env));
}

function run(label, command, args, cwd, env) {
  console.log(`[setup] ${label}`);
  const processResult = spawnSync(command, args, {
    cwd,
    env,
    stdio: "inherit",
    windowsHide: true,
  });
  if (processResult.error) {
    throw new Error(`${label} failed: ${processResult.error.message}`);
  }
  if (processResult.status !== 0) {
    throw new Error(`${label} failed with exit code ${processResult.status ?? 1}`);
  }
}

const bun = process.execPath;
const cargo = process.platform === "win32" ? "cargo.exe" : "cargo";

try {
  let env = setupEnvironment();
  if (process.platform === "win32" && !windowsToolchainReady(env)) {
    const workspaceSetup = resolve(root, "..", "scripts", "setup", "setup-development.ps1");
    if (!existsSync(workspaceSetup)) {
      throw new Error(
        "Windows native tools are missing. Install MSVC, Windows SDK, CMake, and Ninja " +
        "as described in CONTRIBUTING.md, then run setup again. " +
        "The optional portable installer belongs to the Zenith workspace and is not in this clone.",
      );
    }

    run(
      "install portable Windows toolchain",
      "powershell.exe",
      [
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        workspaceSetup,
        "-SkipProjectDependencies",
      ],
      root,
      process.env,
    );
    env = setupEnvironment();
    if (!windowsToolchainReady(env)) {
      throw new Error(
        "Portable Windows setup finished without cl.exe, link.exe, rc.exe, cmake.exe, and ninja.exe.",
      );
    }
  }

  run("check Bun", bun, ["--version"], root, env);
  run("check Rust", cargo, ["--version"], root, env);
  if (process.platform === "win32") {
    run("check MSVC", "cl.exe", [], root, env);
    run("check MSVC linker", "where.exe", ["link.exe"], root, env);
    run("check Windows SDK", "where.exe", ["rc.exe"], root, env);
    run("check CMake", "cmake.exe", ["--version"], root, env);
    run("check Ninja", "ninja.exe", ["--version"], root, env);
  }
  run("install frontend dependencies", bun, ["install", "--frozen-lockfile"], frontendRoot, env);

  for (const manifest of [
    "crates/relay-core/Cargo.toml",
    "relay-server/Cargo.toml",
    "src-tauri/Cargo.toml",
  ]) {
    run(`fetch ${manifest}`, cargo, ["fetch", "--locked", "--manifest-path", manifest], root, env);
  }

  if (wantsBrowsers) {
    run("install Playwright Chromium", bun, ["x", "playwright", "install", "chromium"], frontendRoot, env);
  }

  console.log("[setup] ready — run `bun scripts/setup/start-dev.mjs` to start Relay.");
} catch (error) {
  console.error(`[setup] ${error instanceof Error ? error.message : String(error)}`);
  if (error instanceof Error && error.message.includes("Rust")) {
    console.error("Install Rust with rustup, then run setup again.");
  }
  process.exit(1);
}
