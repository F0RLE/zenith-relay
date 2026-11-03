import { spawnSync } from "node:child_process";
import { repoRoot, withZenithRustEnv } from "../lib/tauri-env.mjs";

const cargoArgs = process.argv.slice(2);
if (cargoArgs.length === 0) {
  console.error("Usage: bun scripts/build/cargo.mjs <cargo arguments>");
  process.exit(2);
}

const command = process.platform === "win32" ? "cargo.exe" : "cargo";
const cargoResult = spawnSync(command, cargoArgs, {
  cwd: repoRoot(),
  env: withZenithRustEnv(),
  shell: false,
  stdio: "inherit",
  windowsHide: true,
});

if (cargoResult.error) {
  console.error(`[cargo] ${cargoResult.error.message}`);
  process.exit(1);
}

process.exit(cargoResult.status ?? 1);
