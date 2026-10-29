import { spawnSync } from "node:child_process";
import { repoRoot, tauriInvocation, withZenithRustEnv } from "../lib/tauri-env.mjs";
import { tauriDevOptions } from "./tauri-dev-args.mjs";

let options;
try {
  options = tauriDevOptions(process.argv.slice(2));
} catch (error) {
  console.error(`[dev] ${error.message}`);
  process.exit(2);
}
if (options.help) {
  console.log("Usage: bun scripts/build/tauri-dev.mjs [--watch | --no-watch] [Tauri dev options]");
  console.log("Default: Rust auto-restart is OFF; frontend hot reload remains enabled.");
  console.log("Apply Rust edits by stopping and restarting this command after active requests finish.");
  console.log("Use --watch only when agents connect through a separate stable Relay instance.");
  process.exit(0);
}
console.log(options.watch
  ? "[dev] Rust auto-restart is ON. Rebuilds interrupt requests through this instance."
  : "[dev] Rust auto-restart is OFF. Frontend hot reload stays enabled; restart manually to apply Rust edits.");
const invocation = tauriInvocation(options.args);
const buildResult = spawnSync(invocation.command, invocation.args, {
  cwd: repoRoot(),
  env: withZenithRustEnv(),
  shell: invocation.shell,
  stdio: "inherit",
});

if (buildResult.error) console.error(`[dev] ${buildResult.error.message}`);
process.exit(buildResult.status ?? 1);
