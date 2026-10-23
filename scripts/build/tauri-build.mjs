import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { repoRoot, tauriInvocation, usesDefaultWindowsReleaseOutput, withZenithRustEnv } from "../lib/tauri-env.mjs";

const cliArgs = process.argv.slice(2);
const buildArgs = ["build", "--config", "src-tauri/tauri.conf.json", ...cliArgs];
const isLocalWindowsRelease = usesDefaultWindowsReleaseOutput(cliArgs);
const releaseExecutablePath = join(repoRoot(), "src-tauri", "target", "release", "zenith-relay.exe");
const productionMarkerPath = `${releaseExecutablePath}.production.sha256`;

if (isLocalWindowsRelease) rmSync(productionMarkerPath, { force: true });

if (!process.env.TAURI_SIGNING_PRIVATE_KEY) {
  buildArgs.push(
    "--config",
    JSON.stringify({
      bundle: {
        createUpdaterArtifacts: false,
      },
    }),
  );
}

const invocation = tauriInvocation(buildArgs);
const buildResult = spawnSync(invocation.command, invocation.args, {
  cwd: repoRoot(),
  env: withZenithRustEnv(),
  shell: invocation.shell,
  stdio: "inherit",
  windowsHide: true,
});

if (buildResult.error) {
  console.error(`[build] Failed to start Tauri: ${buildResult.error.message}`);
  process.exit(1);
}

const buildStatus = buildResult.status ?? 1;
if (buildStatus === 0 && isLocalWindowsRelease && existsSync(releaseExecutablePath)) {
  const executableSha256 = createHash("sha256")
    .update(readFileSync(releaseExecutablePath))
    .digest("hex")
    .toUpperCase();
  writeFileSync(productionMarkerPath, `${executableSha256}\n`, "ascii");
}

process.exit(buildStatus);
