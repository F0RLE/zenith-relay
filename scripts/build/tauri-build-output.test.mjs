import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { conciseBuildOutput, runLoggedTauriBuild } from "./tauri-build-output.mjs";

test("successful builds show budgets and completion instead of asset listings", () => {
  const output = "dist/assets/app.js 240 kB\n✓ built in 4s\ninitialJavaScript: 148 KiB gzip\n    Finished `release` profile in 8m\n";
  expect(conciseBuildOutput(output)).toBe("✓ built in 4s\ninitialJavaScript: 148 KiB gzip\n    Finished `release` profile in 8m");
});

test("compiler failures retain actionable diagnostics while the log owns backtraces", () => {
  const output = [
    "warning: synthetic warning", "  --> src/main.rs:1:1", "thread 'rustc' panicked",
    "stack backtrace:", "  0: 0x0000 - synthetic frame", "    at /rustc/compiler.rs:1:1", "",
    "error: the compiler unexpectedly panicked", "query stack during panic:",
    "#0 [fn_abi_of_instance_raw] synthetic query", "  process didn't exit successfully: `rustc --synthetic`",
  ].join("\n");
  const summary = conciseBuildOutput(output, true);
  expect(summary).toContain("warning: synthetic warning\n  --> src/main.rs:1:1");
  expect(summary).toContain("error: the compiler unexpectedly panicked");
  expect(summary).toContain("#0 [fn_abi_of_instance_raw] synthetic query");
  expect(summary).not.toContain("synthetic frame");
  expect(summary).not.toContain("rustc --synthetic");
});

test("successful builds retain warning context and failures retain dist diagnostics", () => {
  const warning = "warning: synthetic warning\n  --> src/main.rs:1:1\n   | example\n\n";
  expect(conciseBuildOutput(`${warning}dist/assets/app.js 240 kB\n✓ built in 4s`)).toBe(`${warning}✓ built in 4s`);
  expect(conciseBuildOutput("dist/assets/app.js (1:2): synthetic error", true)).toContain("synthetic error");
});

test("a missing build command returns its startup failure", async () => {
  const directory = mkdtempSync(join(tmpdir(), "relay-build-start-"));
  try {
    const result = await runLoggedTauriBuild({
      command: join(directory, "missing-build-tool.exe"), args: [],
    }, { shell: false, windowsHide: true }, {
      logPath: join(directory, "build.log"), progressIntervalMs: 0,
      log() {}, error() {},
    });
    expect(result.error?.code).toBe("ENOENT");
    expect(result.status).not.toBe(0);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test.each([0, 7])("logged builds preserve both streams and exit status %i", async status => {
  const directory = mkdtempSync(join(tmpdir(), "relay-build-log-"));
  const logPath = join(directory, "build.log");
  const messages = [];
  try {
    const result = await runLoggedTauriBuild({
      command: process.execPath,
      args: ["-e", `console.log("synthetic stdout"); console.error("synthetic stderr"); process.exit(${status});`],
    }, { shell: false, windowsHide: true }, {
      logPath, progressIntervalMs: 0,
      log: message => messages.push(message), error: message => messages.push(message),
    });
    expect(result.status).toBe(status);
    expect(result.error).toBeUndefined();
    expect(readFileSync(logPath, "utf8")).toContain("synthetic stdout");
    expect(readFileSync(logPath, "utf8")).toContain("synthetic stderr");
    expect(messages[0]).toContain(logPath);
    if (status !== 0) expect(messages[1]).toContain("synthetic stderr");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("build diagnostics stay bound to the opened log when its pathname is replaced", async () => {
  const directory = mkdtempSync(join(tmpdir(), "relay-build-replaced-"));
  const logPath = join(directory, "build.log");
  const messages = [];
  try {
    const result = await runLoggedTauriBuild({
      command: process.execPath,
      args: ["-e", 'console.error("error: original diagnostic"); process.exit(7);'],
    }, { shell: false, windowsHide: true }, {
      logPath, progressIntervalMs: 0,
      log(message) {
        messages.push(message);
        renameSync(logPath, join(directory, "original.log"));
        writeFileSync(logPath, "error: replacement diagnostic");
      },
      error: message => messages.push(message),
    });
    expect(result.status).toBe(7);
    expect(messages[1]).toContain("error: original diagnostic");
    expect(messages[1]).not.toContain("replacement diagnostic");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
