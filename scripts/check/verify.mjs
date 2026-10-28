import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { repoRoot } from "../lib/tauri-env.mjs";

export function parseVerifyArgs(args) {
  const options = { quick: false, help: false, base: undefined };
  for (let index = 0; index < args.length; index++) {
    const arg = args[index];
    if (arg === "--quick") options.quick = true;
    else if (arg === "--help" || arg === "-h") options.help = true;
    else if (arg === "--base") {
      const value = args[++index];
      if (!value || value.startsWith("-")) throw new Error("--base requires a Git reference.");
      options.base = value;
    } else throw new Error(`Unknown argument: ${arg}`);
  }
  return options;
}

export function verificationSteps({ quick = false, base } = {}) {
  const steps = [];
  const add = (label, args, directory = ".") => steps.push({ label, args, directory });
  const cargo = (label, args) => add(label, ["scripts/build/cargo.mjs", ...args]);
  add("Script tests", ["test", "./scripts/check", "./scripts/lib", "./scripts/maintenance", "./scripts/build"]);
  add("Documentation", ["scripts/check/check-docs.mjs"]);
  for (const name of ["agent-guardrails", "duplicate-code"]) {
    add(name, [`scripts/check/check-${name}.mjs`, ...(base ? [base] : [])]);
  }
  add("Desktop UI tests", ["run", "test:unit"], "src");
  add("Desktop UI build and budgets", ["run", "build"], "src");
  add("Web preview tests", ["run", "test"], "web");
  add("Web preview build", ["run", "build"], "web");
  if (!quick) {
    for (const manifest of ["crates/relay-core/Cargo.toml", "src-tauri/Cargo.toml"]) {
      cargo(`Rust formatting: ${manifest}`, ["fmt", "--manifest-path", manifest, "--all", "--", "--check"]);
      cargo(`Rust lint: ${manifest}`, ["clippy", "--manifest-path", manifest, "--all-targets", "--locked", "--", "-D", "warnings"]);
      cargo(`Rust tests: ${manifest}`, ["test", "--manifest-path", manifest, "--locked", ...(manifest.startsWith("src-tauri/") ? ["--", "--test-threads=1"] : [])]);
    }
    add("Desktop browser tests", ["run", "test:e2e"], "src");
    add("Web browser tests", ["run", "test:e2e"], "web");
    add("Desktop dependency audit", ["audit"], "src");
    add("Web dependency audit", ["audit"], "web");
    for (const lockfile of ["crates/relay-core/Cargo.lock", "src-tauri/Cargo.lock"]) {
      cargo(`Rust dependency audit: ${lockfile}`, ["audit", "--file", lockfile]);
    }
  }
  return steps;
}

export function runVerification(steps, { root = repoRoot(), spawn = spawnSync, log = console.log } = {}) {
  for (const [index, step] of steps.entries()) {
    log(`[verify ${index + 1}/${steps.length}] ${step.label}`);
    const result = spawn(process.execPath, step.args, {
      cwd: join(root, step.directory), env: process.env, stdio: "inherit",
      shell: false, windowsHide: true,
    });
    if (result.error || result.status !== 0) {
      log(`[verify] Failed: ${step.label}${result.error ? `: ${result.error.message}` : result.signal ? ` (${result.signal})` : ` (exit ${result.status})`}`);
      return result.status > 0 ? result.status : 1;
    }
  }
  log("[verify] All selected checks passed.");
  return 0;
}

if (import.meta.main) {
  try {
    const options = parseVerifyArgs(process.argv.slice(2));
    if (options.help) {
      console.log("Usage: bun run verify [--quick] [--base <git-ref>]");
      console.log("Default: scripts, docs, guardrails, UI/web builds, Rust, browsers and dependency audits.");
      console.log("--quick: scripts, docs, guardrails and UI/web checks only; skips Rust, browsers and audits.");
      console.log("Prepare locked dependencies, native tools, Chromium and cargo-audit first; audits need network access.");
    } else {
      console.log(options.quick ? "[verify] Quick local checks; not the full release gate." : "[verify] Full local verification.");
      process.exitCode = runVerification(verificationSteps(options));
    }
  } catch (error) {
    console.error(`[verify] ${error.message}`);
    process.exitCode = 2;
  }
}
