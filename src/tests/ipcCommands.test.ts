import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";

test("renderer IPC calls reference registered desktop commands", () => {
  const native = readFileSync(new URL("../../src-tauri/src/desktop.rs", import.meta.url), "utf8");
  const registry = /tauri::generate_handler!\[([\s\S]*?)\]/u.exec(native)?.[1];
  if (!registry) throw new Error("Desktop command registry was not found");
  const registered = new Set(registry.split(",").map(path => path.trim().split("::").at(-1)));
  const commands = new Set<string>();
  const transpiler = new Bun.Transpiler({ loader: "ts" });

  for (const file of [
    new URL("../src/features/relay/api/commands.ts", import.meta.url),
    new URL("../src/platform/desktop.ts", import.meta.url),
  ]) {
    const source = transpiler.transformSync(readFileSync(file, "utf8"));
    const calls = [...source.matchAll(/\b(?:invoke|tauriInvoke)\(\s*["']([a-z0-9_]+)["']/gu)];
    expect(calls.length).toBeGreaterThan(0);
    for (const call of calls) commands.add(call[1]!);
  }

  expect([...commands].filter(command => !registered.has(command)).sort()).toEqual([]);
});
