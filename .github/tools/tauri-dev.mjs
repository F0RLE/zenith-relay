import { spawnSync } from "node:child_process";
import { repoRoot, tauriInvocation, withZenithRustEnv } from "./tauri-env.mjs";

const invocation = tauriInvocation(["dev", "--config", "src-tauri/tauri.conf.json"]);
const result = spawnSync(invocation.command, invocation.args, {
  cwd: repoRoot(),
  env: withZenithRustEnv(),
  shell: invocation.shell,
  stdio: "inherit",
});

process.exit(result.status ?? 1);
