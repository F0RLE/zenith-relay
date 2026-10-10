import { spawnSync } from "node:child_process";
import { repoRoot, tauriInvocation, withZenithRustEnv } from "../lib/tauri-env.mjs";

const invocation = tauriInvocation(["dev", "--config", "src-tauri/tauri.conf.json"]);
const buildResult = spawnSync(invocation.command, invocation.args, {
  cwd: repoRoot(),
  env: withZenithRustEnv(),
  shell: invocation.shell,
  stdio: "inherit",
});

process.exit(buildResult.status ?? 1);
