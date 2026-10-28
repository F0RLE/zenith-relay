import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { repoRoot, tauriInvocation, usesDefaultWindowsReleaseOutput, withZenithRustEnv } from "../lib/tauri-env.mjs";
import { tauriBuildArgs } from "./tauri-build-args.mjs";

const cliArgs = process.argv.slice(2);
const buildArgs = tauriBuildArgs(cliArgs, { signingKeyPresent: Boolean(process.env.TAURI_SIGNING_PRIVATE_KEY) });
const isLocalWindowsRelease = usesDefaultWindowsReleaseOutput(cliArgs);
const releaseExecutablePath = join(repoRoot(), "src-tauri", "target", "release", "zenith-relay.exe");
const productionMarkerPath = `${releaseExecutablePath}.production.sha256`;

if (isLocalWindowsRelease) rmSync(productionMarkerPath, { force: true });

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
if (buildResult.signal) console.error(`[build] Tauri was interrupted by ${buildResult.signal}; no successful build stamp was written.`);
if (buildStatus === 0 && isLocalWindowsRelease) {
  if (!existsSync(releaseExecutablePath)) {
    console.error(`[build] Tauri returned success without the expected executable: ${releaseExecutablePath}`);
    process.exit(1);
  }
  const executableSha256 = createHash("sha256")
    .update(readFileSync(releaseExecutablePath))
    .digest("hex")
    .toUpperCase();
  writeFileSync(productionMarkerPath, `${executableSha256}\n`, "ascii");
}

process.exit(buildStatus);
