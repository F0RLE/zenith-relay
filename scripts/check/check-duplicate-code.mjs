import { spawnSync } from "node:child_process";

const ignore = [
  "**/tests/**",
  "**/tests.rs",
  "**/*_test.rs",
  "**/node_modules/**",
  "**/.build/**",
  "**/protocol/management.rs",
  "**/protocol/management/account.rs",
  "**/app/account_runtime.rs",
  "**/store/usage.rs",
  "**/store/usage/**",
  "**/local_pool/commands/runtime.rs",
  "**/local_pool/commands/automations.rs",
  "**/local_pool/state.rs",
  "**/local_pool/store.rs",
  "**/store/migrations.rs",
  "**/local_pool/store/telemetry_db/migrations.rs",
  "**/local_pool/store/telemetry_db/migrations/**",
  "**/usage_writer.rs",
  // The server and desktop vaults intentionally keep separate implementations:
  // their file formats, limits, and filesystem hardening policies differ.
  "relay-server/src/store/vault.rs",
  "src-tauri/src/local_pool/store/vault.rs",
].join(",");

function resolveBaseRef() {
  const requested = process.argv.slice(2).find((arg) => arg && !arg.startsWith("-"));
  if (requested) return requested;
  if (process.env.GITHUB_EVENT_NAME === "pull_request" && process.env.GITHUB_BASE_REF) {
    return `origin/${process.env.GITHUB_BASE_REF}`;
  }
  if (process.env.GITHUB_EVENT_NAME === "push") return "HEAD^";
  return "HEAD";
}

function run(command, args) {
  const result = spawnSync(command, args, { stdio: "inherit" });
  if (result.error) {
    console.error(result.error.message);
    process.exit(1);
  }
  if (result.status !== 0) process.exit(result.status ?? 1);
}

const baseRef = resolveBaseRef();
const verify = spawnSync("git", ["rev-parse", "--verify", "--quiet", `${baseRef}^{commit}`], {
  encoding: "utf8",
});
if (verify.status !== 0) {
  console.error(`Duplicate-code check requires a valid base ref, got '${baseRef}'.`);
  process.exit(1);
}

function check(paths, format) {
  console.log(`Checking ${format} clones introduced after ${baseRef}`);
  run(process.execPath, [
    "x",
    "--bun",
    "jscpd@5.3.1",
    ...paths,
    "--format",
    format,
    "--min-tokens",
    "100",
    "--min-lines",
    "10",
    "--skip-comments",
    "--ignore",
    ignore,
    "--baseline-from-ref",
    baseRef,
    "--fail-on-new-clones",
    "0",
    "--reporters",
    "console",
    "--no-colors",
    "--no-tips",
  ]);
}

check(["crates", "relay-server", "src-tauri"], "rust");
check(["src/src"], "typescript");
