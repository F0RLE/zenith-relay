import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { join } from "node:path";
import { repoRoot } from "../lib/tauri-env.mjs";

const root = repoRoot();
const setupScript = join(root, "scripts", "setup", "setup-dev.mjs");
const tauriScript = join(root, "scripts", "build", "tauri-dev.mjs");
const wantsBrowsers = process.argv.includes("--with-browsers");
const helpRequested = process.argv.includes("--help") || process.argv.includes("-h");

if (helpRequested) {
  console.log("Usage: bun scripts/setup/start-dev.mjs [--with-browsers]");
  console.log("Install locked dependencies and start the Relay desktop app.");
  console.log("Add --with-browsers when Playwright Chromium is needed for e2e tests.");
  process.exit(0);
}

if (!process.versions.bun) {
  console.error("Run this script with Bun: bun scripts/setup/start-dev.mjs");
  process.exit(1);
}

function run(label, script, args = []) {
  if (!existsSync(script)) {
    console.error(`[dev] ${label} script is missing: ${script}`);
    process.exit(1);
  }

  console.log(`[dev] ${label}`);
  const result = spawnSync(process.execPath, [script, ...args], {
    cwd: root,
    env: process.env,
    stdio: "inherit",
    windowsHide: true,
  });

  if (result.error) {
    console.error(`[dev] ${label} failed: ${result.error.message}`);
    process.exit(1);
  }
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

run("install dependencies", setupScript, wantsBrowsers ? ["--with-browsers"] : []);
run("start Relay", tauriScript);
