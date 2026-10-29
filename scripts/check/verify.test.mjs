import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { EventEmitter } from "node:events";
import { join } from "node:path";
import { parseVerifyArgs, runVerification, verificationSteps } from "./verify.mjs";

function completedChild(result = { status: 0 }) {
  const child = new EventEmitter();
  queueMicrotask(() => result.error ? child.emit("error", result.error) : child.emit("close", result.status, result.signal));
  return child;
}

describe("repository verification command", () => {
  test("root and frontend commands use the same repository runner", () => {
    const root = JSON.parse(readFileSync(new URL("../../package.json", import.meta.url), "utf8"));
    const frontend = JSON.parse(readFileSync(new URL("../../src/package.json", import.meta.url), "utf8"));
    expect(root.scripts.verify).toBe("bun scripts/check/verify.mjs");
    expect(frontend.scripts.verify).toBe("bun ../scripts/check/verify.mjs");
    const workflow = readFileSync(new URL("../../.github/workflows/build.yml", import.meta.url), "utf8");
    // CI owns separate browser/native/audit jobs; it must not recurse into all of them.
    expect(workflow).not.toMatch(/bun run verify/);
    expect(workflow).toContain("bun run test:unit");
    expect(workflow).toContain("bun ../scripts/build/cargo.mjs test --manifest-path src-tauri/Cargo.toml --locked -- --test-threads=1");
  });

  test("full checks include both Rust packages, browsers and all dependency audits", () => {
    const steps = verificationSteps();
    for (const manifest of ["crates/relay-core/Cargo.toml", "src-tauri/Cargo.toml"]) {
      for (const command of ["fmt", "clippy", "test"]) {
        expect(steps.some(({ args }) => args[1] === command && args.includes(manifest))).toBe(true);
      }
      expect(steps.some(({ args }) => args[1] === "audit" && args.includes(manifest.replace("Cargo.toml", "Cargo.lock")))).toBe(true);
    }
    for (const directory of ["src", "web"]) {
      expect(steps.some((step) => step.directory === directory && step.args.includes("test:e2e"))).toBe(true);
      expect(steps.some((step) => step.directory === directory && step.args[0] === "audit")).toBe(true);
    }
    const desktop = steps.find(({ args }) => args[1] === "test" && args.includes("src-tauri/Cargo.toml"));
    expect(desktop.args).toContain("--test-threads=1");
  });

  test("quick mode retains local checks and makes its narrower scope explicit", () => {
    const quick = verificationSteps({ quick: true, base: "origin/release/1.1.5" });
    expect(quick).toHaveLength(8);
    expect(quick.filter(({ args }) => args.includes("origin/release/1.1.5"))).toHaveLength(2);
    expect(quick.some(({ args }) => args.includes("scripts/build/cargo.mjs") || args.includes("audit") || args.includes("test:e2e"))).toBe(false);
    expect(quick.filter(({ args }) => args.includes("build"))).toHaveLength(2);
  });

  test("rejects unknown flags and missing base references", () => {
    expect(parseVerifyArgs(["--quick", "--base", "HEAD"])).toEqual({ quick: true, help: false, base: "HEAD", jobs: 2 });
    expect(parseVerifyArgs(["--jobs", "1"]).jobs).toBe(1);
    expect(parseVerifyArgs(["--jobs", "3"]).jobs).toBe(3);
    expect(parseVerifyArgs(["-h"]).help).toBe(true);
    for (const args of [["--skip-tests"], ["--base"], ["--base", "--quick"], ["--jobs"], ["--jobs", "0"], ["--jobs", "4"], ["--jobs", "2junk"]]) {
      expect(() => parseVerifyArgs(args)).toThrow();
    }
  });

  test("anchors working directories and preserves arguments without a shell", async () => {
    const root = join(process.cwd(), "synthetic checkout with spaces");
    const calls = [];
    const code = await runVerification(verificationSteps({ quick: true }), {
      root, log() {}, spawn(command, args, options) { calls.push({ command, args, options }); return completedChild(); },
    });
    expect(code).toBe(0);
    expect(calls).toHaveLength(8);
    expect(calls[0].options.cwd).toBe(root);
    expect(calls.at(-1).options.cwd).toBe(join(root, "web"));
    expect(calls.every(({ command, options }) => command === process.execPath && options.shell === false && options.windowsHide)).toBe(true);
  });

  test("stops at the first preflight failure, interruption or spawn error", async () => {
    for (const result of [{ status: 7 }, { status: null, signal: "SIGTERM" }, { error: new Error("synthetic missing tool") }]) {
      let calls = 0;
      const code = await runVerification(verificationSteps({ quick: true }), {
        log() {}, spawn() { calls++; return completedChild(calls === 2 ? result : { status: 0 }); },
      });
      expect(calls).toBe(2);
      expect(code).toBe(result.status === 7 ? 7 : 1);
    }
  });

  test("preflight finishes before bounded parallel lanes, with no same-lane overlap", async () => {
    const calls = [];
    const active = new Set();
    let maximum = 0;
    const steps = [
      { label: "preflight", args: ["preflight"], directory: ".", lane: "preflight" },
      ...["desktop", "web", "rust"].flatMap((lane) => [1, 2].map((number) => ({ label: `${lane}${number}`, args: [lane], directory: ".", lane }))),
    ];
    const result = await runVerification(steps, { jobs: 2, log() {}, spawn(_command, args) {
      const lane = args[0];
      expect(active.has(lane)).toBe(false);
      if (lane !== "preflight") expect(calls[0]).toBe("preflight");
      expect(active.has("preflight")).toBe(false);
      calls.push(lane);
      active.add(lane);
      maximum = Math.max(maximum, active.size);
      const child = new EventEmitter();
      queueMicrotask(() => { active.delete(lane); child.emit("close", 0, null); });
      return child;
    } });
    expect(result).toBe(0);
    expect(maximum).toBe(2);
    expect(calls).toHaveLength(7);
    expect(active.size).toBe(0);
  });

  test("parallel failure waits for started work and schedules no remaining steps", async () => {
    const calls = [];
    const steps = ["desktop", "web", "rust"].flatMap((lane) => [1, 2].map((number) => ({ label: `${lane}${number}`, args: [lane], directory: ".", lane })));
    let finished = 0;
    const result = await runVerification(steps, { log() {}, spawn(_command, args) {
      calls.push(args[0]);
      const child = new EventEmitter();
      setTimeout(() => { finished++; child.emit("close", args[0] === "desktop" ? 7 : 0, null); }, args[0] === "desktop" ? 0 : 10);
      return child;
    } });
    expect(result).toBe(7);
    expect(calls).toEqual(["desktop", "web"]);
    expect(finished).toBe(2);
  });

  test("single-job mode preserves lane ordering and synchronous spawn exceptions fail closed", async () => {
    const calls = [];
    const steps = ["desktop", "web", "rust"].flatMap((lane) => [1, 2].map((number) => ({ label: `${lane}${number}`, args: [lane], directory: ".", lane })));
    expect(await runVerification(steps, { jobs: 1, log() {}, spawn(_command, args) { calls.push(args[0]); return completedChild(); } })).toBe(0);
    expect(calls).toEqual(["desktop", "desktop", "web", "web", "rust", "rust"]);
    expect(await runVerification(steps, { log() {}, spawn() { throw new Error("synthetic spawn failure"); } })).toBe(1);
    for (const jobs of [0, 4, 1.5]) await expect(runVerification([], { jobs, log() {} })).rejects.toThrow();
  });

  test("native commands stay in one lane and each frontend builds before its browser suite", () => {
    const steps = verificationSteps();
    expect(steps.filter(({ args }) => args.includes("scripts/build/cargo.mjs")).every(({ lane }) => lane === "rust")).toBe(true);
    for (const [directory, lane] of [["src", "desktop"], ["web", "web"]]) {
      const frontend = steps.filter((step) => step.directory === directory);
      expect(frontend.every((step) => step.lane === lane)).toBe(true);
      expect(frontend.findIndex(({ args }) => args.includes("build"))).toBeLessThan(frontend.findIndex(({ args }) => args.includes("test:e2e")));
    }
  });
});
