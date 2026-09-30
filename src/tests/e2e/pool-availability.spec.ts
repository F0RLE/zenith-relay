import { expect, test, type Page } from "../bun-playwright";
import type { OperationalStatus, RuntimeSnapshot } from "../../src/features/relay/api/types";
import { installTauriMock, emitTauriEvent } from "./tauri-mock";

type Preview = "complete" | "empty" | "partial" | "stale";
const mixedStatuses: OperationalStatus[] = ["unavailable", "quotaWait", "disabled", "rotation", "unavailable", "rotation"];

async function setPoolState(
  page: Page,
  mode: "local" | "remote",
  statuses: OperationalStatus[],
  preview: Preview = "complete",
  hasModels = true,
) {
  await page.evaluate(async ({ mode, statuses, preview, hasModels }) => {
    const scope = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
    };
    const invoke = scope.__TAURI_INTERNALS__.invoke.bind(scope.__TAURI_INTERNALS__);
    const snapshotCommand = mode === "local" ? "get_local_runtime_state" : "get_remote_server_state";
    const orderCommand = mode === "local" ? "get_local_runtime_order" : "get_remote_runtime_order";
    const runtime = await invoke(snapshotCommand) as RuntimeSnapshot;
    runtime.sources = [];
    runtime.accounts = runtime.accounts.slice(0, statuses.length).map((account, index) => ({
      ...account,
      label: `Fixture ${index + 1}`,
      identityHint: `Fixture ${index + 1}`,
      inPool: true,
      enabled: statuses[index] !== "disabled",
      draining: false,
      operationalStatus: statuses[index],
      authState: statuses[index] === "unavailable"
        ? { state: "requires_reauth", reason: "invalid_grant" }
        : { state: "active" },
      health: "healthy",
      secretAvailable: true,
      proxyAvailable: true,
      lastErrorCode: null,
      models: ["gpt-5.4"],
      quotaRefreshStatus: "updated",
      quota: {
        ...account.quota,
        primary: account.quota.primary ? { ...account.quota.primary, availableBasisPoints: statuses[index] === "quotaWait" ? 0 : 7200 } : null,
        secondary: account.quota.secondary ? { ...account.quota.secondary, availableBasisPoints: 6400 } : null,
        limitReached: statuses[index] === "quotaWait",
        error: null,
      },
    }));
    runtime.gateway.candidateCount = statuses.filter((status) => status === "rotation").length;
    runtime.gateway.visibleModelIds = hasModels ? ["gpt-5.4"] : [];
    runtime.gateway.routingOrder = runtime.accounts.map((account, index) => ({
      candidateId: account.id,
      kind: "oauth_account" as const,
      available: preview !== "stale" && account.operationalStatus === "rotation",
      nextForNewRequest: preview === "complete" && index === statuses.indexOf("rotation"),
      activityRevision: 0,
      inFlight: 0,
      activeRequestCount: 0,
      activeModels: [],
      lastUsedAtMs: null,
      nextRetryAtMs: null,
      halfOpen: false,
      dispatches: 0,
    })).filter((candidate, index) => preview !== "empty"
      && (preview !== "partial" || statuses[index] === "unavailable"));
    scope.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      if (command === snapshotCommand) return structuredClone(runtime);
      if (command === orderCommand) return structuredClone(runtime.gateway.routingOrder);
      return invoke(command, args, options);
    };
  }, { mode, statuses, preview, hasModels });
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect(page.locator('.pool-member-card[data-member-label="Fixture 1"]')).toBeVisible();
}

for (const mode of ["local", "remote"] as const) {
  for (const preview of ["empty", "partial", "stale"] as const) {
    test(`${mode} mixed pool stays usable with ${preview} route telemetry`, async ({ page }) => {
      await installTauriMock(page, { mode, locale: "en", accountCount: 6, usagePresent: false });
      await page.goto("/");
      await page.getByRole("button", { name: "Pool", exact: true }).click();
      await setPoolState(page, mode, mixedStatuses, preview);

      await expect(page.locator(".pool-routing-alert")).toHaveCount(0);
      await expect(page.locator(".pool-current-route")).toHaveText("Waiting for the first request");
      if (mode === "local") await expect(page.getByRole("button", { name: "Connect", exact: true })).toBeEnabled();
      const order = ["Fixture 4", "Fixture 6", "Fixture 2", "Fixture 1", "Fixture 5", "Fixture 3"];
      await expect(page.locator(".pool-member-name")).toHaveText(order);

      await page.getByRole("button", { name: "Connections", exact: true }).click();
      await expect(page.locator(".account-card .account-identity > strong")).toHaveText(order);
    });
  }
}

test("unavailable pool explains counts and deduplicates localized sign-in errors", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await installTauriMock(page, { mode: "local", locale: "ru", accountCount: 6, usagePresent: false, theme: "dark" });
  await page.goto("/");
  await page.getByRole("button", { name: "Пул", exact: true }).click();
  await setPoolState(page, "local", ["unavailable", "quotaWait", "disabled", "unavailable"]);
  const alert = page.locator(".pool-routing-alert");
  await expect(alert).toContainText("Ожидают квоту: 1; недоступны: 2; отключены: 1");
  await expect(alert).not.toContainText("auth_invalid_grant");
  await expect(alert).not.toContainText("no_eligible_source");
  expect((await alert.innerText()).match(/Выполнен выход или сменена учётная запись/g)).toHaveLength(1);
  expect(await alert.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("unavailable-mobile.png"), animations: "disabled" });
});

test("quota waits and disabled models have distinct pool warnings", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", accountCount: 6, usagePresent: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await setPoolState(page, "local", ["quotaWait", "quotaWait"]);
  const alert = page.locator(".pool-routing-alert");
  await expect(alert).toContainText("Waiting for quota: 2; unavailable: 0; disabled: 0");
  await expect(alert).not.toContainText("Sign-in");
  await expect(alert).not.toContainText("no_eligible_source");

  await setPoolState(page, "local", ["rotation", "unavailable"], "complete", false);
  await expect(alert.locator("strong")).toHaveText("No available models");
  await expect(alert).not.toContainText("Sign-in");
});

test("mixed pool groups remain readable on a narrow screen", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await installTauriMock(page, { mode: "local", locale: "ru", accountCount: 6, usagePresent: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Пул", exact: true }).click();
  await setPoolState(page, "local", mixedStatuses);
  await expect(page.locator(".pool-routing-alert")).toHaveCount(0);
  await expect(page.locator(".pool-current-route")).toHaveText("Следующий кандидат: Fixture 4");
  expect(await page.locator(".pool-member-card").evaluateAll((cards) => cards.every((card) => {
    const rect = card.getBoundingClientRect();
    return rect.left >= 0 && rect.right <= innerWidth && card.scrollWidth <= card.clientWidth;
  }))).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("mixed-mobile.png"), animations: "disabled" });
});

test("pool uses explicit preview and rejects activity from a replaced runtime", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", accountCount: 1, quotaAvailable: true, usagePresent: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.evaluate(async () => {
    const scope = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
      __POOL_RUNTIME__: RuntimeSnapshot;
    };
    const invoke = scope.__TAURI_INTERNALS__.invoke.bind(scope.__TAURI_INTERNALS__);
    const runtime = await invoke("get_local_runtime_state") as RuntimeSnapshot;
    runtime.gateway.routingOrder = [...runtime.gateway.routingOrder ?? []]
      .sort((a, b) => Number(a.kind === "api_source") - Number(b.kind === "api_source"))
      .map((item) => ({ ...item, runtimeId: 2, activityRevision: 0, nextForNewRequest: item.candidateId === "source_synthetic" }));
    scope.__POOL_RUNTIME__ = runtime;
    scope.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      if (command === "get_local_runtime_state") return structuredClone(scope.__POOL_RUNTIME__);
      if (command === "get_local_runtime_order") return structuredClone(scope.__POOL_RUNTIME__.gateway.routingOrder);
      return invoke(command, args, options);
    };
  });
  await emitTauriEvent(page, "zenith-state-changed", null);
  const summary = page.locator(".pool-controls");
  await expect(summary).toContainText("Next candidate: Example compatible API");
  const event = (runtimeId: number, revision: number, candidateId: string, count: number) => emitTauriEvent(page, "zenith-runtime-activity", {
    runtimeId, revision, candidateId, inFlight: count, activeRequestCount: count,
    activeModels: count ? [{ model: "gpt-5.4", requestCount: count }] : [],
  });
  await event(2, 1, "account_synthetic", 1);
  await expect(summary).toContainText("Active now: Personal Plus");
  await expect(summary).not.toContainText("Next candidate");
  await event(1, 100, "account_synthetic", 0);
  await event(2, 2, "source_synthetic", 1);
  await expect(summary.locator("[data-active-request-count]")).toHaveAttribute("data-active-request-count", "2");

  // A replacement can send its first event before its first snapshot arrives.
  await event(3, 1, "account_synthetic", 1);
  await event(2, 200, "source_synthetic", 2);
  await expect(summary.locator("[data-active-request-count]")).toHaveAttribute("data-active-request-count", "1");
  await expect(page.locator('[data-member-label="Example compatible API"]')).toHaveAttribute("data-current", "false");
  await page.evaluate(() => {
    const runtime = (window as unknown as { __POOL_RUNTIME__: RuntimeSnapshot }).__POOL_RUNTIME__;
    runtime.gateway.routingOrder = runtime.gateway.routingOrder?.map((item) => ({
      ...item, runtimeId: 3, activityRevision: 1,
      inFlight: item.kind === "oauth_account" ? 1 : 0,
      activeRequestCount: item.kind === "oauth_account" ? 1 : 0,
      activeModels: item.kind === "oauth_account" ? [{ model: "gpt-5.4", requestCount: 1 }] : [],
    })) ?? [];
  });
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect(summary).toContainText("Next candidate: Example compatible API");
  await expect(summary).toContainText("Active now: Personal Plus");
});
