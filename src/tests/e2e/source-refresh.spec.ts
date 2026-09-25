import { expect, test, type Page } from "../bun-playwright";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

async function statsCalls(page: Page) {
  return page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: { force?: boolean } }>;
  }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "get_local_source_stats" || call.command === "get_remote_source_stats"));
}

for (const mode of ["local", "remote", "zenith"] as const) {
  test(`${mode} projects background source observations without another provider read`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true });
    await page.goto("/");
    if (mode !== "zenith") await page.getByRole("button", { name: "Pool", exact: true }).click();
    const metrics = page.locator(mode === "zenith" ? ".direct-api-metrics" : ".pool-source-stats").first();
    await expect(metrics).toContainText("$42.50");
    const before = (await statsCalls(page)).length;
    await page.evaluate(() => {
      const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
      const original = internals.invoke.bind(internals);
      internals.invoke = async (command, args, options) => {
        const result = await original(command, args, options);
        if (command === "get_local_runtime_state" || command === "get_remote_server_state") {
          const snapshot = result as { sources: Array<{ providerStats?: unknown }> };
          snapshot.sources[0]!.providerStats = {
            provider: "zenith", status: "available", balanceMicroUsd: 17_000_000,
            spentMicroUsd: null, requests: null, totalTokens: null, asOfMs: Date.now(),
          };
        }
        return result;
      };
    });
    if (mode === "remote") await page.evaluate(() => window.dispatchEvent(new Event("focus")));
    else await emitTauriEvent(page, "zenith-state-changed", null);
    await expect(metrics).toContainText("$17.00");
    expect((await statsCalls(page)).length).toBe(before);
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.getByRole("button", { name: mode === "zenith" ? "Overview" : "Pool", exact: true }).click();
    await expect(metrics).toContainText("$17.00");
    expect((await statsCalls(page)).length).toBe(before);
  });
}

for (const mode of ["local", "remote"] as const) {
  test(`${mode} distinguishes model, quota, and balance refresh evidence from routing health`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.evaluate(() => {
      const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
      const original = internals.invoke.bind(internals);
      internals.invoke = async (command, args, options) => {
        const result = await original(command, args, options);
        if (command === "get_local_runtime_state" || command === "get_remote_server_state") {
          const snapshot = result as { sources: Array<{ refreshState?: unknown; providerStats?: unknown }>; accounts: Array<{ refreshState?: unknown }> };
          snapshot.sources[0]!.refreshState = { models: "stale", balance: "unsupported" };
          snapshot.sources[0]!.providerStats = {
            provider: "unsupported", status: "unsupported", balanceMicroUsd: null,
            spentMicroUsd: null, requests: null, totalTokens: null,
          };
          snapshot.accounts[0]!.refreshState = { models: "fresh", quota: "stale" };
        }
        return result;
      };
    });
    if (mode === "remote") await page.evaluate(() => window.dispatchEvent(new Event("focus")));
    else await emitTauriEvent(page, "zenith-state-changed", null);
    const source = page.locator('[data-member-kind="source"]').first();
    await expect(source.locator('[data-refresh-models]')).toHaveCount(0);
    await expect(source.locator('[data-metric="balance"]')).toContainText("No balance API");
    const account = page.locator('[data-member-kind="account"]').first();
    await expect(account.locator('[data-quota-refresh]')).toHaveCount(0);
    await expect(account.locator('[data-models-refresh]')).toHaveCount(0);
    await expect(source).toHaveAttribute("data-member-kind", "source");
  });
}

test("overview force refresh is a one-shot intent, not a permanent polling flag", async ({ page }) => {
  await installTauriMock(page, { mode: "zenith", locale: "en", populated: true });
  await page.goto("/");
  await expect(page.locator(".direct-api-metrics")).toContainText("$42.50");
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect.poll(async () => (await statsCalls(page)).at(-1)?.args.force).toBe(true);
  const before = (await statsCalls(page)).length;
  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const original = internals.invoke.bind(internals);
    internals.invoke = async (command, args, options) => {
      const result = await original(command, args, options);
      if (command === "get_local_runtime_state") {
        (result as { sources: Array<{ refreshRevision: number }> }).sources[0]!.refreshRevision = 2;
      }
      return result;
    };
  });
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect.poll(async () => (await statsCalls(page)).length).toBeGreaterThan(before);
  expect((await statsCalls(page)).at(-1)?.args.force).toBe(false);
});
