import { spawn as spawnProcess } from "node:child_process";
import { join } from "node:path";
import { repoRoot } from "../lib/tauri-env.mjs";

export function parseVerifyArgs(args) {
  const options = { quick: false, help: false, base: undefined, jobs: 2 };
  for (let index = 0; index < args.length; index++) {
    const arg = args[index];
    if (arg === "--quick") options.quick = true;
    else if (arg === "--help" || arg === "-h") options.help = true;
    else if (arg === "--jobs") {
      const value = args[++index];
      if (!/^[1-3]$/.test(value ?? "")) throw new Error("--jobs requires 1, 2 or 3 independent check lanes.");
      options.jobs = Number(value);
    }
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
  const add = (label, args, directory = ".", lane = "preflight") => steps.push({ label, args, directory, lane });
  const cargo = (label, args) => add(label, ["scripts/build/cargo.mjs", ...args], ".", "rust");
  add("Script tests", ["test", "./scripts/check", "./scripts/lib", "./scripts/maintenance", "./scripts/build"]);
  add("Documentation", ["scripts/check/check-docs.mjs"]);
  for (const name of ["agent-guardrails", "duplicate-code"]) {
    add(name, [`scripts/check/check-${name}.mjs`, ...(base ? [base] : [])]);
  }
  add("Desktop UI tests", ["run", "test:unit"], "src", "desktop");
  add("Desktop UI build and budgets", ["run", "build"], "src", "desktop");
  add("Web preview tests", ["run", "test"], "web", "web");
  add("Web preview build", ["run", "build"], "web", "web");
  if (!quick) {
    for (const manifest of ["crates/relay-core/Cargo.toml", "src-tauri/Cargo.toml"]) {
      cargo(`Rust formatting: ${manifest}`, ["fmt", "--manifest-path", manifest, "--all", "--", "--check"]);
      cargo(`Rust lint: ${manifest}`, ["clippy", "--manifest-path", manifest, "--all-targets", "--locked", "--", "-D", "warnings"]);
      cargo(`Rust tests: ${manifest}`, ["test", "--manifest-path", manifest, "--locked", ...(manifest.startsWith("src-tauri/") ? ["--", "--test-threads=1"] : [])]);
    }
    add("Desktop browser tests", ["run", "test:e2e"], "src", "desktop");
    add("Web browser tests", ["run", "test:e2e"], "web", "web");
    add("Desktop dependency audit", ["audit"], "src", "desktop");
    add("Web dependency audit", ["audit"], "web", "web");
    for (const lockfile of ["crates/relay-core/Cargo.lock", "src-tauri/Cargo.lock"]) {
      cargo(`Rust dependency audit: ${lockfile}`, ["audit", "--file", lockfile]);
    }
  }
  return steps;
}

export async function runVerification(steps, { root = repoRoot(), spawn = spawnProcess, log = console.log, jobs = 2 } = {}) {
  if (!Number.isInteger(jobs) || jobs < 1 || jobs > 3) throw new Error("Check concurrency must be between 1 and 3.");
  const started = performance.now();
  const runStep = async (step, index) => {
    const stepStarted = performance.now();
    log(`[verify ${index + 1}/${steps.length}] ${step.label}`);
    const result = await new Promise((resolve) => {
      try {
        const child = spawn(process.execPath, step.args, {
          cwd: join(root, step.directory), env: process.env, stdio: "inherit",
          shell: false, windowsHide: true,
        });
        child.once("error", (error) => resolve({ error }));
        child.once("close", (status, signal) => resolve({ status, signal }));
      } catch (error) {
        resolve({ error });
      }
    });
    if (result.error || result.status !== 0) {
      log(`[verify] Failed: ${step.label}${result.error ? `: ${result.error.message}` : result.signal ? ` (${result.signal})` : ` (exit ${result.status})`}`);
      return result.status > 0 ? result.status : 1;
    }
    log(`[verify] Passed: ${step.label} (${((performance.now() - stepStarted) / 1000).toFixed(1)}s)`);
    return 0;
  };
  const lanes = new Map();
  for (const [index, step] of steps.entries()) {
    if (!step.lane || step.lane === "preflight") {
      const status = await runStep(step, index);
      if (status !== 0) return status;
    } else {
      if (!lanes.has(step.lane)) lanes.set(step.lane, []);
      lanes.get(step.lane).push({ step, index });
    }
  }
  let failure = 0;
  const pending = [...lanes.values()];
  let next = 0;
  const worker = async () => {
    while (!failure && next < pending.length) {
      const lane = pending[next++];
      for (const { step, index } of lane) {
        if (failure) return;
        const status = await runStep(step, index);
        if (status !== 0) { failure ||= status; return; }
      }
    }
  };
  // Finish already-started processes after a failure; do not orphan Cargo or its compiler children.
  await Promise.all(Array.from({ length: Math.min(jobs, pending.length) }, worker));
  if (failure) return failure;
  log(`[verify] All selected checks passed (${((performance.now() - started) / 1000).toFixed(1)}s).`);
  return 0;
}

if (import.meta.main) {
  try {
    const options = parseVerifyArgs(process.argv.slice(2));
    if (options.help) {
      console.log("Usage: bun run verify [--quick] [--base <git-ref>] [--jobs <1-3>]");
      console.log("Default: scripts, docs, guardrails, UI/web builds, Rust, browsers and dependency audits.");
      console.log("--quick: scripts, docs, guardrails and UI/web checks only; skips Rust, browsers and audits.");
      console.log("--jobs: independent Desktop, web and Rust lanes (default 2; 1 runs sequentially).");
      console.log("Prepare locked dependencies, native tools, Chromium and cargo-audit first; audits need network access.");
    } else {
      console.log(options.quick ? "[verify] Quick local checks; not the full release gate." : "[verify] Full local verification.");
      process.exitCode = await runVerification(verificationSteps(options), { jobs: options.jobs });
    }
  } catch (error) {
    console.error(`[verify] ${error.message}`);
    process.exitCode = 2;
  }
}
