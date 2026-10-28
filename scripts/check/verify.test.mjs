import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { parseVerifyArgs, runVerification, verificationSteps } from "./verify.mjs";

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
    expect(parseVerifyArgs(["--quick", "--base", "HEAD"])).toEqual({ quick: true, help: false, base: "HEAD" });
    expect(parseVerifyArgs(["-h"]).help).toBe(true);
    for (const args of [["--skip-tests"], ["--base"], ["--base", "--quick"]]) {
      expect(() => parseVerifyArgs(args)).toThrow();
    }
  });

  test("anchors working directories and preserves arguments without a shell", () => {
    const root = join(process.cwd(), "synthetic checkout with spaces");
    const calls = [];
    const code = runVerification(verificationSteps({ quick: true }), {
      root, log() {}, spawn(command, args, options) { calls.push({ command, args, options }); return { status: 0 }; },
    });
    expect(code).toBe(0);
    expect(calls).toHaveLength(8);
    expect(calls[0].options.cwd).toBe(root);
    expect(calls.at(-1).options.cwd).toBe(join(root, "web"));
    expect(calls.every(({ command, options }) => command === process.execPath && options.shell === false && options.windowsHide)).toBe(true);
  });

  test("stops at the first failure, interruption or spawn error", () => {
    for (const result of [{ status: 7 }, { status: null, signal: "SIGTERM" }, { error: new Error("synthetic missing tool") }]) {
      let calls = 0;
      const code = runVerification(verificationSteps({ quick: true }), {
        log() {}, spawn() { calls++; return calls === 2 ? result : { status: 0 }; },
      });
      expect(calls).toBe(2);
      expect(code).toBe(result.status === 7 ? 7 : 1);
    }
  });
});
