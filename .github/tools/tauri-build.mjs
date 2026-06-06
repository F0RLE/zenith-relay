import { spawnSync } from "node:child_process";
import { repoRoot, tauriInvocation, withZenithRustEnv } from "./tauri-env.mjs";

const args = ["build", "--config", "src-tauri/tauri.conf.json"];

if (!process.env.TAURI_SIGNING_PRIVATE_KEY) {
  args.push(
    "--config",
    JSON.stringify({
      bundle: {
        createUpdaterArtifacts: false,
      },
    }),
  );
}

const invocation = tauriInvocation(args);
const result = spawnSync(invocation.command, invocation.args, {
  cwd: repoRoot(),
  env: withZenithRustEnv(),
  shell: invocation.shell,
  stdio: "inherit",
});

process.exit(result.status ?? 1);
