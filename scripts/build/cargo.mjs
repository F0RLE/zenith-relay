import { spawnSync } from "node:child_process";
import { repoRoot, withZenithRustEnv } from "../lib/tauri-env.mjs";

const args = process.argv.slice(2);
if (args.length === 0) {
  console.error("Usage: bun scripts/build/cargo.mjs <cargo arguments>");
  process.exit(2);
}

const command = process.platform === "win32" ? "cargo.exe" : "cargo";
const result = spawnSync(command, args, {
  cwd: repoRoot(),
  env: withZenithRustEnv(),
  shell: false,
  stdio: "inherit",
  windowsHide: true,
});

if (result.error) {
  console.error(`[cargo] ${result.error.message}`);
  process.exit(1);
}

process.exit(result.status ?? 1);
