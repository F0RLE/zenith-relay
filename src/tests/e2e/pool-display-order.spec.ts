import { expect, test } from "../bun-playwright";
import type { PoolRoutingMode, RuntimeSnapshot } from "../../src/features/relay/api/types";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

for (const mode of ["automatic", "in_order", "round_robin", "weighted_round_robin", "failover"] satisfies PoolRoutingMode[]) {
  for (const width of [2451, 1160, 390]) {
    test(`pool cards follow ${mode} preference and highlight only active work at ${width}px`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width, height: 844 });
      await installTauriMock(page, { locale: "en", theme: width === 1160 ? "light" : "dark", populated: true, accountCount: 6 });
      await page.goto("/");
      await page.evaluate(async (mode) => {
        const scope = window as unknown as {
          __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
        };
        const invoke = scope.__TAURI_INTERNALS__.invoke.bind(scope.__TAURI_INTERNALS__);
        const runtime = await invoke("get_local_runtime_state") as RuntimeSnapshot;
        runtime.sources = [];
        runtime.accounts.push({ ...structuredClone(runtime.accounts[0]!), id: "account_synthetic_7" });
        runtime.accounts.forEach((account, index) => {
          account.label = account.identityHint = `Member ${index + 1}`;
          account.inPool = true;
          account.enabled = index !== 0;
          account.draining = false;
          account.operationalStatus = index === 0 ? "disabled" : index === 1 ? "unavailable" : index === 2 ? "quotaWait" : "rotation";
          account.authState = index < 2 ? { state: "requires_reauth", reason: "invalid_grant" } : { state: "active" };
          account.lastErrorCode = index < 2 ? "invalid_grant" : null;
          account.secretAvailable = account.proxyAvailable = true;
          account.quotaRefreshStatus = "updated";
          account.quota.error = null;
          account.quota.limitReached = index === 2;
        });
        runtime.gateway.poolRouting = {
          ...runtime.gateway.poolRouting!, version: 3, mode,
          members: runtime.accounts.map((account) => ({ kind: "account", id: account.id, weight: 1, maxConcurrency: 0 })),
        };
        runtime.gateway.routingOrder = runtime.accounts.map((account, index) => ({
          candidateId: account.id, kind: "oauth_account", available: index >= 3 && index < 6,
          selectionRank: index === 4 ? 0 : index === 5 ? 1 : index === 3 ? 2 : null,
          quotaWait: index === 2, nextForNewRequest: index === 4,
          inFlight: index === 3 ? 1 : 0, activeRequestCount: index === 3 ? 1 : 0,
          activityRevision: 20, lastUsedAtMs: null, dispatches: 0, halfOpen: index === 5,
          nextRetryAtMs: index === 2 || index === 6 ? Date.now() + 60_000 : null,
        }));
        scope.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
          if (command === "get_local_runtime_state") return structuredClone(runtime);
          if (command === "get_local_runtime_order") return structuredClone(runtime.gateway.routingOrder);
          if (command === "test_advance_pool_preference") {
            for (const candidate of runtime.gateway.routingOrder!) {
              if (candidate.selectionRank === 0) { candidate.selectionRank = 1; candidate.nextForNewRequest = false; }
              else if (candidate.selectionRank === 1) { candidate.selectionRank = 0; candidate.nextForNewRequest = true; }
            }
            return;
          }
          return invoke(command, args, options);
        };
      }, mode);
      await emitTauriEvent(page, "zenith-state-changed", null);
      await page.getByRole("button", { name: "Pool", exact: true }).click();
      const cards = page.locator(".pool-member-card");
      await expect(cards.locator(".pool-member-name")).toHaveText(["Member 4", "Member 5", "Member 6", "Member 3", "Member 7", "Member 2", "Member 1"]);
      await expect(cards.nth(1)).toHaveAttribute("data-next", "true");
      await expect(cards.nth(0)).toHaveAttribute("data-current", "true");
      await expect(page.locator(".pool-member-card.current")).toHaveCount(1);
      await expect(cards.locator(".pool-member-activity")).toHaveCount(0);
      await expect(cards.last().locator(".pool-member-kind-icon")).toHaveAttribute("data-status", "disabled");
      await expect(cards.nth(4).locator(".pool-member-kind-icon")).toHaveAttribute("data-status", "warning");
      await expect(cards.nth(4).locator(".pool-member-kind-icon")).toHaveAttribute("aria-label", /Waiting to retry/);
      const layout = await cards.evaluateAll((elements) => {
        const boxes = elements.map((element) => element.getBoundingClientRect());
        const lastRow = boxes.filter((box) => box.top === boxes.at(-1)?.top);
        return {
          rowMajor: boxes.every((box, index) => index === 0 || box.top > boxes[index - 1].top
            || (box.top === boxes[index - 1].top && box.left > boxes[index - 1].left)),
          lastRowStartsLeft: Math.abs(lastRow[0].left - Math.min(...boxes.map((box) => box.left))) < 1,
          backgrounds: new Set(elements.map((element) => getComputedStyle(element).backgroundColor)).size,
          activeBorder: getComputedStyle(elements[0]).borderColor !== getComputedStyle(elements[1]).borderColor,
          neutralCandidates: getComputedStyle(elements[1]).borderColor === getComputedStyle(elements[2]).borderColor,
          textDirection: elements.every((element) => getComputedStyle(element).direction === "ltr"),
          fits: document.documentElement.scrollWidth <= innerWidth,
        };
      });
      expect(layout).toEqual({ rowMajor: true, lastRowStartsLeft: true, backgrounds: 1, activeBorder: true, neutralCandidates: true, textDirection: true, fits: true });
      if (mode === "automatic") await page.screenshot({ path: testInfo.outputPath("pool-order.png"), fullPage: true });

      await page.evaluate(async () => {
        const scope = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<unknown> } };
        await scope.__TAURI_INTERNALS__.invoke("test_advance_pool_preference");
      });
      await emitTauriEvent(page, "zenith-state-changed", null);
      await expect(cards.locator(".pool-member-name")).toHaveText(["Member 4", "Member 6", "Member 5", "Member 3", "Member 7", "Member 2", "Member 1"]);
      const saves = await page.evaluate(() => (window as unknown as {
        __TAURI_TEST_INVOKES__: { command: string }[];
      }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "update_local_routing"));
      expect(saves).toHaveLength(0);
    });
  }
}

for (const width of [1160, 390]) {
  test(`pool activity refreshes preference through cooldown, recovery and draining at ${width}px`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height: 844 });
    await installTauriMock(page, { locale: "ru", theme: width === 390 ? "dark" : "light", accountCount: 2, quotaAvailable: true });
    await page.goto("/");
    await page.evaluate(async () => {
      const scope = window as unknown as {
        __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
        __POOL_RUNTIME__: RuntimeSnapshot;
        __POOL_ORDER_READS__: number;
      };
      const invoke = scope.__TAURI_INTERNALS__.invoke.bind(scope.__TAURI_INTERNALS__);
      const runtime = await invoke("get_local_runtime_state") as RuntimeSnapshot;
      runtime.accounts.forEach((account, index) => {
        account.label = account.identityHint = `Account ${index + 1}`;
        account.operationalStatus = "rotation";
        account.quota.limitReached = false;
      });
      runtime.gateway.poolRouting = {
        ...runtime.gateway.poolRouting!, version: 3, mode: "round_robin",
        members: [
          ...runtime.accounts.map((account) => ({ kind: "account" as const, id: account.id, weight: 1, maxConcurrency: 0 })),
          { kind: "source", id: runtime.sources[0]!.id, weight: 1, maxConcurrency: 0 },
        ],
      };
      runtime.gateway.routingOrder = ["account_synthetic", "account_synthetic_2", "source_synthetic::responses", "source_synthetic::chat"].map((candidateId, index) => ({
        candidateId, kind: index < 2 ? "oauth_account" : "api_source", runtimeId: 1, activityRevision: 20,
        available: true, selectionRank: Math.min(index, 2), nextForNewRequest: index === 0,
        inFlight: 0, activeRequestCount: 0, activeModels: [], lastUsedAtMs: null,
        nextRetryAtMs: null, halfOpen: false, dispatches: 0,
      }));
      scope.__POOL_RUNTIME__ = runtime;
      scope.__POOL_ORDER_READS__ = 0;
      scope.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
        if (command === "get_local_runtime_state") return structuredClone(runtime);
        if (command === "get_local_runtime_order") {
          scope.__POOL_ORDER_READS__ += 1;
          return structuredClone(runtime.gateway.routingOrder);
        }
        return invoke(command, args, options);
      };
    });
    await emitTauriEvent(page, "zenith-state-changed", null);
    await page.getByRole("button", { name: "Пул", exact: true }).click();
    const cards = page.locator(".pool-member-card");
    const firstAccount = page.locator('.pool-member-card[data-member-label="Account 1"]');
    const source = page.locator('.pool-member-card[data-member-kind="source"]');
    await expect(cards.locator(".pool-member-name")).toHaveText(["Account 1", "Account 2", "Example compatible API"]);
    const activity = (candidateId: string, revision: number, count: number) => emitTauriEvent(page, "zenith-runtime-activity", {
      runtimeId: 1, revision, candidateId, inFlight: count, activeRequestCount: count,
      activeModels: count ? [{ model: "gpt-5.4", requestCount: count }] : [],
    });
    await page.evaluate(() => {
      const runtime = (window as unknown as { __POOL_RUNTIME__: RuntimeSnapshot }).__POOL_RUNTIME__;
      runtime.gateway.routingOrder!.forEach((candidate, index) => {
        candidate.activityRevision = 21;
        candidate.selectionRank = index === 0 ? 2 : index === 1 ? 0 : 1;
        candidate.nextForNewRequest = index === 1;
        candidate.inFlight = candidate.activeRequestCount = index === 0 ? 1 : 0;
      });
    });
    await activity("account_synthetic", 21, 1);
    await expect(firstAccount).toHaveAttribute("data-current", "true");
    // No full state event: activity must refresh the authoritative rank before the 5s fallback.
    await expect(cards.locator(".pool-member-name")).toHaveText(["Account 1", "Account 2", "Example compatible API"], { timeout: 2_000 });
    await expect(cards.nth(1)).toHaveAttribute("data-next", "true", { timeout: 2_000 });

    await page.evaluate(() => {
      const runtime = (window as unknown as { __POOL_RUNTIME__: RuntimeSnapshot }).__POOL_RUNTIME__;
      runtime.gateway.routingOrder!.forEach((candidate, index) => {
        candidate.activityRevision = 23;
        candidate.available = index !== 0;
        candidate.selectionRank = index === 0 ? null : index === 1 ? 1 : 0;
        candidate.nextForNewRequest = index >= 2;
        candidate.nextRetryAtMs = index === 0 ? Date.now() + 60_000 : null;
        candidate.inFlight = candidate.activeRequestCount = index < 2 ? 0 : index === 2 ? 1 : 2;
      });
    });
    const readsBeforeBurst = await page.evaluate(() => (window as unknown as { __POOL_ORDER_READS__: number }).__POOL_ORDER_READS__);
    await activity("account_synthetic", 22, 0);
    await activity("source_synthetic::responses", 23, 1);
    await activity("source_synthetic::chat", 23, 2);
    await expect(source).toHaveAttribute("data-current", "true");
    await expect(source.locator(".pool-member-runtime-meta > div").last().locator("strong")).toHaveText("3");
    await expect(cards.locator(".pool-member-name")).toHaveText(["Example compatible API", "Account 2", "Account 1"], { timeout: 2_000 });
    await expect(firstAccount.locator(".pool-member-kind-icon")).toHaveAttribute("data-status", "warning");
    await expect(firstAccount).toHaveAttribute("data-current", "false");
    expect(await page.evaluate(() => (window as unknown as { __POOL_ORDER_READS__: number }).__POOL_ORDER_READS__) - readsBeforeBurst).toBe(1);

    await page.evaluate(() => {
      const runtime = (window as unknown as { __POOL_RUNTIME__: RuntimeSnapshot }).__POOL_RUNTIME__;
      runtime.gateway.routingOrder!.forEach((candidate, index) => {
        candidate.activityRevision = 24;
        candidate.available = true;
        candidate.selectionRank = index === 0 ? 0 : index === 1 ? 2 : 1;
        candidate.nextForNewRequest = index === 0;
        candidate.nextRetryAtMs = null;
        candidate.halfOpen = index === 0;
        if (index === 0) candidate.inFlight = candidate.activeRequestCount = 1;
      });
    });
    await activity("account_synthetic", 24, 1);
    await expect(cards.locator(".pool-member-name")).toHaveText(["Account 1", "Example compatible API", "Account 2"], { timeout: 2_000 });
    await expect(firstAccount.locator(".account-runtime-line")).toHaveCount(0);
    await expect(firstAccount.locator(".pool-member-kind-icon")).toHaveAttribute("aria-label", /Проверка восстановления/);
    await expect(page.locator(".pool-member-card.current .pool-member-name")).toHaveText(["Account 1", "Example compatible API"]);
    await expect(cards.locator(".pool-member-activity")).toHaveCount(0);
    await page.screenshot({ path: testInfo.outputPath("pool-active-recovery.png"), fullPage: true });

    await page.evaluate(() => {
      const runtime = (window as unknown as { __POOL_RUNTIME__: RuntimeSnapshot }).__POOL_RUNTIME__;
      runtime.accounts[0]!.draining = true;
      runtime.accounts[0]!.subscription.planType = "pro";
      runtime.accounts[0]!.quota.primary = { ...runtime.accounts[0]!.quota.primary!, availableBasisPoints: 4200 };
    });
    await emitTauriEvent(page, "zenith-state-changed", null);
    await expect(cards.locator(".pool-member-name")).toHaveText(["Example compatible API", "Account 1", "Account 2"]);
    await expect(firstAccount).toHaveAttribute("data-current", "true");
    await expect(firstAccount.getByText("Pro 200", { exact: true })).toBeVisible();
    await expect(firstAccount.locator(".pool-member-card-quota")).toContainText("42%");
    await expect(firstAccount.locator(".pool-member-kind-icon")).toHaveAttribute("data-status", "disabled");

    // The backend mock still contains active counts: release tombstones must win.
    await activity("account_synthetic", 25, 0);
    await activity("source_synthetic::responses", 26, 0);
    await activity("source_synthetic::chat", 27, 0);
    await expect(page.locator(".pool-member-card.current")).toHaveCount(0);
    await expect.poll(async () => page.evaluate(() => (window as unknown as { __POOL_ORDER_READS__: number }).__POOL_ORDER_READS__)).toBeGreaterThan(readsBeforeBurst + 2);
    await expect(page.locator(".pool-member-card.current")).toHaveCount(0);
  });
}
