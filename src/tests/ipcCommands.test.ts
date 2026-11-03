import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { initializeTauriMock } from "./e2e/tauri-mock/runtime";
import type { MockOptions } from "./e2e/tauri-mock/options";

function isolatedMock(options: MockOptions, preferences = new Map<string, string>()) {
  const window = {} as {
    __TAURI_INTERNALS__: { invoke: (command: string) => Promise<unknown> };
  };
  // Match Playwright's serialization boundary: no module closure is available
  // inside a new WebView, even when importing the function works in this runner.
  runInNewContext(`(${initializeTauriMock.toString()})(options)`, {
    window,
    options,
    localStorage: {
      getItem: (key: string) => preferences.get(key) ?? null,
      setItem: (key: string, value: string) => preferences.set(key, value),
    },
    structuredClone,
    URL,
    setTimeout,
    clearTimeout,
  });
  return { invoke: window.__TAURI_INTERNALS__.invoke, preferences };
}

test("browser mock remains serializable and isolates returned runtime snapshots", async () => {
  const { invoke } = isolatedMock({ locale: "ru", accountCount: 2, sourceCount: 1 });
  expect(await invoke("get_system_locale")).toBe("ru");
  const first = await invoke("get_local_runtime_state") as { accounts: unknown[]; sources: unknown[] };
  expect(first.accounts).toHaveLength(2);
  expect(first.sources).toHaveLength(1);
  first.accounts.length = 0;
  const second = await invoke("get_local_runtime_state") as typeof first;
  expect(second.accounts).toHaveLength(2);
});

test("browser mock reload retains preferences and explicit startup failures", async () => {
  const preferences = new Map([["relay.profileSwitchBackupPrompt", "0"]]);
  const { invoke } = isolatedMock({ profileSwitchBackupPrompt: true, runtimeState: "error" }, preferences);
  expect(preferences.get("relay.profileSwitchBackupPrompt")).toBe("0");
  await expect(invoke("get_local_runtime_state")).rejects.toMatchObject({ code: "invalid_state" });
});

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
