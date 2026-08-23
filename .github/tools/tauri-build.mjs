import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { repoRoot, tauriInvocation, withZenithRustEnv } from "./tauri-env.mjs";

const cliArgs = process.argv.slice(2);
const args = ["build", ...cliArgs, "--config", "src-tauri/tauri.conf.json"];
const isLocalWindowsRelease =
  process.platform === "win32" && !cliArgs.includes("--debug") && !cliArgs.includes("--target");
const executable = join(repoRoot(), "src-tauri", "target", "release", "zenith-relay.exe");
const productionHash = `${executable}.production.sha256`;

if (isLocalWindowsRelease) rmSync(productionHash, { force: true });

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

const status = result.status ?? 1;
if (status === 0 && isLocalWindowsRelease) {
  const hash = createHash("sha256").update(readFileSync(executable)).digest("hex").toUpperCase();
  writeFileSync(productionHash, `${hash}\n`, "ascii");
}

process.exit(status);
