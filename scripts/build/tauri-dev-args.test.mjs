import { expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { tauriDevOptions } from "./tauri-dev-args.mjs";

test("default development preserves the serving Rust process", () => {
  expect(tauriDevOptions()).toEqual({
    watch: false, help: false,
    args: ["dev", "--config", "src-tauri/tauri.conf.json", "--no-watch"],
  });
});

test("Rust auto-restart requires explicit opt-in", () => {
  const options = tauriDevOptions(["--watch", "--verbose"]);
  expect(options.watch).toBe(true);
  expect(options.args).toEqual(["dev", "--config", "src-tauri/tauri.conf.json", "--verbose"]);
});

test("explicit no-watch is forwarded once", () => {
  expect(tauriDevOptions(["--no-watch"]).args.filter((arg) => arg === "--no-watch")).toHaveLength(1);
});

test("conflicting watcher modes fail instead of silently restarting", () => {
  expect(() => tauriDevOptions(["--watch", "--no-watch"])).toThrow("not both");
});

test("runner and application arguments do not change the watcher mode", () => {
  const options = tauriDevOptions(["--", "--watch", "--", "--help"]);
  expect(options.watch).toBe(false);
  expect(options.help).toBe(false);
  expect(options.args.slice(-4)).toEqual(["--", "--watch", "--", "--help"]);
});

test("other Tauri options are preserved", () => {
  expect(tauriDevOptions(["--features", "example", "--config", "custom.json"]).args.slice(-4))
    .toEqual(["--features", "example", "--config", "custom.json"]);
});

test.each(["--help", "-h"])("development %s does not start the toolchain", (flag) => {
  const script = fileURLToPath(new URL("./tauri-dev.mjs", import.meta.url));
  const result = spawnSync(process.execPath, [script, flag], {
    cwd: tmpdir(), encoding: "utf8", windowsHide: true,
  });
  expect(result.status, result.stderr).toBe(0);
  expect(result.stdout).toContain("Rust auto-restart is OFF");
  expect(result.stdout).not.toContain("[dev]");
});

test("root and frontend packages expose the explicit watch command", () => {
  for (const path of ["../../package.json", "../../src/package.json"]) {
    const pkg = JSON.parse(readFileSync(new URL(path, import.meta.url), "utf8"));
    expect(pkg.scripts["app:dev"]).toEndWith("tauri-dev.mjs");
    expect(pkg.scripts["app:dev:watch"]).toEndWith("tauri-dev.mjs --watch");
  }
});
