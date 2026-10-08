import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { repoRoot, tauriInvocation, withZenithRustEnv } from "../lib/tauri-env.mjs";

const cliArgs = process.argv.slice(2);
const buildArgs = ["build", ...cliArgs, "--config", "src-tauri/tauri.conf.json"];
const isInformationInvocation = cliArgs.some((argument) =>
  ["--help", "-h", "--version", "-V"].includes(argument),
);
const isLocalWindowsRelease =
  process.platform === "win32" &&
  !isInformationInvocation &&
  !cliArgs.includes("--debug") &&
  !cliArgs.includes("--target");
const executable = join(repoRoot(), "src-tauri", "target", "release", "zenith-relay.exe");
const productionHash = `${executable}.production.sha256`;

if (isLocalWindowsRelease) rmSync(productionHash, { force: true });

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
});

const buildStatus = buildResult.status ?? 1;
if (buildStatus === 0 && isLocalWindowsRelease && existsSync(executable)) {
  const executableHash = createHash("sha256")
    .update(readFileSync(executable))
    .digest("hex")
    .toUpperCase();
  writeFileSync(productionHash, `${executableHash}\n`, "ascii");
}

process.exit(buildStatus);
