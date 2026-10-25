import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

test("Proxies retain their last snapshot on re-entry and a failed refresh", async ({ page }) => {
  await installTauriMock(page, { locale: "en", populated: true, proxyCount: 1 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Proxies", exact: true }).click();
  await expect(page.locator(".proxy-storage-row")).toHaveCount(1);
  const region = await page.locator(".proxy-region").textContent();
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.evaluate(() => {
    const target = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
      __FAIL_PROXY_READ__?: () => void;
    };
    const invoke = target.__TAURI_INTERNALS__.invoke.bind(target.__TAURI_INTERNALS__);
    target.__TAURI_INTERNALS__.invoke = (command, args, options) => {
      if (command !== "get_local_proxy_pool") return invoke(command, args, options);
      return new Promise((_, reject) => {
        target.__FAIL_PROXY_READ__ = () => reject({ code: "io", message: "Synthetic proxy read failure" });
      });
    };
  });
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Proxies", exact: true }).click();
  await expect(page.locator(".proxy-storage-row")).toHaveCount(1);
  await expect(page.locator(".proxy-region")).toHaveText(region!);
  await expect(page.locator(".relay-content .data-placeholder")).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => typeof (window as unknown as { __FAIL_PROXY_READ__?: () => void }).__FAIL_PROXY_READ__)).toBe("function");
  await page.evaluate(() => (window as unknown as { __FAIL_PROXY_READ__: () => void }).__FAIL_PROXY_READ__());
  await expect(page.locator(".proxy-storage [role=alert]")).toBeVisible();
  await expect(page.locator(".proxy-storage-row")).toHaveCount(1);
  await expect(page.locator(".proxy-region")).toHaveText(region!);
});

for (const appearance of [
  { locale: "ru", theme: "dark", width: 840, height: 560 },
  { locale: "en", theme: "light", width: 420, height: 720 },
] as const) {
  test(`first visit without saved data has no skeletons in ${appearance.locale}/${appearance.width}`, async ({ page }) => {
    await page.clock.install();
    await installTauriMock(page, { ...appearance, runtimeStateDelayMs: 20_000 });
    await page.setViewportSize({ width: appearance.width, height: appearance.height });
    await page.goto("/");
    await expect(page.locator("#splash-screen")).toHaveCount(0);
    const pool = appearance.locale === "ru" ? "Пул" : "Pool";
    await page.getByRole("button", { name: pool, exact: true }).click();
    await expect(page.locator(".relay-content .data-placeholder")).toHaveCount(0);
    await expect(page.locator(".relay-content")).not.toContainText(appearance.locale === "ru" ? "Загрузка" : "Loading");
    await expect(page.locator(".pool-controls")).toHaveCount(0);
    expect(await page.locator(".relay-content").evaluate((node) => node.scrollWidth <= node.clientWidth + 1)).toBe(true);
    await page.screenshot({ path: `output/playwright/first-visit-${appearance.locale}-${appearance.width}.png` });
    await page.clock.fastForward(20_000);
    await expect(page.locator(".pool-controls")).toBeVisible();
    await expect(page.locator(".relay-content .data-placeholder")).toHaveCount(0);
  });
}

async function savedSnapshots(page: Page, store: string) {
  return page.evaluate(async (name) => {
    if (!(await indexedDB.databases()).some((db) => db.name === "relay.displaySnapshots.v1")) return [];
    return new Promise<Array<{ key: string; value: unknown }>>((resolve, reject) => {
      const open = indexedDB.open("relay.displaySnapshots.v1");
      open.onerror = () => reject(open.error);
      open.onsuccess = () => {
        const request = open.result.transaction(name).objectStore(name).getAll();
        request.onsuccess = () => { resolve(request.result); open.result.close(); };
        request.onerror = () => reject(request.error);
      };
    });
  }, store);
}

async function holdSnapshotReads(page: Page, commands: string[]) {
  await page.addInitScript((held) => {
    const target = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
      __RELEASE_VIEWS__: (fail?: boolean) => void;
    };
    const invoke = target.__TAURI_INTERNALS__.invoke.bind(target.__TAURI_INTERNALS__);
    let pending: Array<(fail: boolean) => void> = [];
    target.__RELEASE_VIEWS__ = (fail = false) => {
      const current = pending;
      pending = [];
      current.forEach((settle) => settle(fail));
    };
    target.__TAURI_INTERNALS__.invoke = (command, args, options) => {
      if (!held.includes(command)) return invoke(command, args, options);
      return new Promise((resolve, reject) => pending.push((fail) => {
        if (fail) reject({ code: "io", message: "Synthetic view read failure" });
        else void invoke(command, args, options).then(resolve, reject);
      }));
    };
  }, commands);
}

async function releaseSnapshotReads(page: Page, fail = false) {
  await page.evaluate((fail) => (window as unknown as { __RELEASE_VIEWS__: (fail: boolean) => void }).__RELEASE_VIEWS__(fail), fail);
}

test("Usage reload hydrates only its selected saved page, not all eight previews", async ({ page }) => {
  await installTauriMock(page, { locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
  const row = await page.locator(".usage-request-table tbody tr").textContent();
  await expect.poll(async () => (await savedSnapshots(page, "usage")).length).toBe(1);
  await page.evaluate(async () => {
    await new Promise<void>((resolve, reject) => {
      const open = indexedDB.open("relay.displaySnapshots.v1");
      open.onerror = () => reject(open.error);
      open.onsuccess = () => {
        const transaction = open.result.transaction("usage", "readwrite");
        const store = transaction.objectStore("usage");
        const read = store.getAll();
        read.onsuccess = () => {
          const first = read.result[0];
          for (let index = 0; index < 7; index += 1) {
            store.put({ ...first, key: `synthetic-unvisited-${index}`, savedAt: Date.now() + index,
              value: { ...first.value, syntheticPadding: "x".repeat(256 * 1024) } });
          }
        };
        transaction.oncomplete = () => { open.result.close(); resolve(); };
        transaction.onerror = () => reject(transaction.error);
      };
    });
  });
  await holdSnapshotReads(page, ["get_local_usage_page"]);
  await page.addInitScript(() => {
    const reads = { keys: [] as string[], bulk: 0 };
    (window as unknown as { __USAGE_SNAPSHOT_READS__: typeof reads }).__USAGE_SNAPSHOT_READS__ = reads;
    const get = IDBObjectStore.prototype.get;
    IDBObjectStore.prototype.get = function (key) {
      if (this.name === "usage") reads.keys.push(String(key));
      return get.call(this, key);
    };
    const getAll = IDBIndex.prototype.getAll;
    IDBIndex.prototype.getAll = function (...args) {
      if (this.objectStore.name === "usage") reads.bulk += 1;
      return getAll.apply(this, args);
    };
  });
  await page.reload();
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-request-table tbody tr")).toHaveText(row!);
  const reads = await page.evaluate(() => (window as unknown as { __USAGE_SNAPSHOT_READS__: { keys: string[]; bulk: number } }).__USAGE_SNAPSHOT_READS__);
  expect(reads.bulk).toBe(0);
  expect(reads.keys).toHaveLength(1);
  expect(reads.keys[0]).not.toContain("synthetic-unvisited");
  await releaseSnapshotReads(page);
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
});

for (const appearance of [
  { locale: "ru", theme: "dark", width: 840, height: 560 },
  { locale: "en", theme: "light", width: 420, height: 720 },
] as const) {
  test(`reload restores real Pool and Usage before native reads in ${appearance.locale}`, async ({ page }) => {
    await installTauriMock(page, { ...appearance, populated: true, quotaAvailable: true });
    await page.setViewportSize({ width: appearance.width, height: appearance.height });
    const poolLabel = appearance.locale === "ru" ? "Пул" : "Pool";
    const usageLabel = appearance.locale === "ru" ? "Использование" : "Usage";
    await page.goto("/");
    await page.getByRole("button", { name: usageLabel, exact: true }).click();
    await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
    // The current selected-page contract includes real period totals.
    await expect(page.locator(".usage-overview")).toBeVisible();
    const summary = await page.locator(".usage-overview").textContent();
    const row = await page.locator(".usage-request-table tbody tr").textContent();
    await page.getByRole("button", { name: poolLabel, exact: true }).click();
    await expect(page.locator(".pool-source-stats").first()).toContainText(/42[,.]50/);
    await expect.poll(async () => (await savedSnapshots(page, "runtime")).length).toBe(1);
    await expect.poll(async () => (await savedSnapshots(page, "usage")).length).toBe(1);
    await holdSnapshotReads(page, ["get_local_runtime_state", "get_local_usage_page"]);
    await page.reload();
    await expect(page.locator("#splash-screen")).toHaveCount(0);
    await page.getByRole("button", { name: poolLabel, exact: true }).click();
    await expect(page.locator(".pool-source-stats").first()).toContainText(/42[,.]50/);
    await expect(page.locator(".pool-controls")).toBeVisible();
    await expect(page.locator(".relay-content [inert]")).toHaveCount(1);
    await expect(page.locator(".data-placeholder")).toHaveCount(0);
    expect(await page.evaluate(() => performance.getEntriesByName("zenith:interactive", "measure"))).toEqual([]);
    await page.screenshot({ path: `output/playwright/previous-pool-${appearance.locale}.png` });
    await page.getByRole("button", { name: usageLabel, exact: true }).click();
    await expect(page.locator(".usage-overview")).toHaveText(summary!);
    await expect(page.locator(".usage-request-table tbody tr")).toHaveText(row!);
    await expect(page.locator(".relay-content")).not.toContainText(appearance.locale === "ru" ? "Загрузка" : "Loading");
    await page.screenshot({ path: `output/playwright/previous-usage-${appearance.locale}.png` });
    await releaseSnapshotReads(page);
    await expect(page.locator(".relay-content [inert]")).toHaveCount(0);
    await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
    await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
  });
}

test("a lightweight saved report does not invent a period summary", async ({ page }) => {
  await installTauriMock(page, { locale: "en", populated: true });
  await page.goto("/");
  await expect(page.getByRole("button", { name: "Stop API", exact: true })).toBeVisible();
  await page.evaluate(() => {
    const internals = (window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: Record<string, unknown>, options?: unknown) => Promise<unknown> };
    }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    internals.invoke = (command, args, options) => invoke(command,
      command === "get_local_usage_page" ? { ...args, includeReport: false } : args, options);
  });
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
  await expect(page.locator(".usage-overview")).toHaveCount(0);
  await expect.poll(async () => (await savedSnapshots(page, "usage")).length).toBe(1);
  await holdSnapshotReads(page, ["get_local_runtime_state", "get_local_usage_page"]);
  await page.reload();
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
  await expect(page.locator(".usage-overview")).toHaveCount(0);
  await releaseSnapshotReads(page);
  await expect(page.locator(".relay-content [inert]")).toHaveCount(0);
});

test("a saved inventory survives failed revalidation; Hosted never shows local cards", async ({ page }) => {
  await installTauriMock(page, { locale: "en" });
  await page.goto("/");
  await expect.poll(async () => (await savedSnapshots(page, "runtime")).length).toBe(1);
  await holdSnapshotReads(page, ["get_local_runtime_state"]);
  await page.reload();
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.locator(".pool-controls")).toBeVisible();
  await releaseSnapshotReads(page, true);
  await expect(page.locator(".pool-controls")).toBeVisible();
  await expect(page.getByRole("button", { name: "Retry", exact: true })).toBeVisible();
  await page.locator(".mode-picker-trigger").click();
  await page.getByRole("menuitemradio", { name: "Relay Hosted", exact: true }).click();
  await expect(page.locator(".pool-controls")).toHaveCount(0);
  await expect(page.locator(".relay-shell")).toHaveAttribute("data-mode", "hosted");
});

test("proxy rows are restored after reload while the native proxy read is pending", async ({ page }) => {
  await installTauriMock(page, { locale: "en", proxyCount: 1 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Proxies", exact: true }).click();
  await expect(page.locator(".proxy-storage-row")).toHaveCount(1);
  const region = await page.locator(".proxy-region").textContent();
  await expect.poll(async () => (await savedSnapshots(page, "proxies")).length).toBe(1);
  await holdSnapshotReads(page, ["get_local_proxy_pool"]);
  await page.reload();
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Proxies", exact: true }).click();
  await expect(page.locator(".proxy-storage-row")).toHaveCount(1);
  await expect(page.locator(".proxy-region")).toHaveText(region!);
  await releaseSnapshotReads(page, true);
  await expect(page.locator(".proxy-storage [role=alert]")).toBeVisible();
  await expect(page.locator(".proxy-storage-row")).toHaveCount(1);
});

test("clearing history removes every saved report before restart", async ({ page }) => {
  await installTauriMock(page, { locale: "en" });
  await page.goto("/");
  await expect(page.locator(".overview-analytics")).toHaveAttribute("aria-busy", "false");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
  await page.getByRole("tab", { name: "Models", exact: true }).click();
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await expect.poll(async () => (await savedSnapshots(page, "usage")).length).toBe(2);
  await page.getByRole("tab", { name: "Sends", exact: true }).click();
  await page.locator(".usage-overflow > summary").click();
  await page.getByRole("menuitem", { name: "Clear logs", exact: true }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Confirm", exact: true }).click();
  await expect(page.getByText("No matching results", { exact: true })).toBeVisible();
  await expect.poll(async () => {
    const saved = await savedSnapshots(page, "usage");
    return saved.length === 1 && (saved[0]?.value as { total: number }).total === 0;
  }).toBe(true);
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem("relay.overviewAnalyticsCache.v1") ?? "[]"))).toEqual([]);
  await holdSnapshotReads(page, ["get_local_usage_page"]);
  await page.reload();
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.getByText("No matching results", { exact: true })).toBeVisible();
  await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(0);
});

test("malformed saved inventory is ignored without blocking a fresh native read", async ({ page }) => {
  await installTauriMock(page, { locale: "en" });
  await page.goto("/");
  await expect.poll(async () => (await savedSnapshots(page, "runtime")).length).toBe(1);
  await page.evaluate(() => new Promise<void>((resolve) => {
    const open = indexedDB.open("relay.displaySnapshots.v1");
    open.onsuccess = () => {
      const transaction = open.result.transaction("runtime", "readwrite");
      transaction.objectStore("runtime").put({ key: "local", savedAt: Date.now(), value: { accounts: [null] } });
      transaction.oncomplete = () => { open.result.close(); resolve(); };
    };
  }));
  await holdSnapshotReads(page, ["get_local_runtime_state"]);
  await page.reload();
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.locator(".pool-controls")).toHaveCount(0);
  await expect(page.locator(".data-placeholder")).toHaveCount(0);
  await releaseSnapshotReads(page);
  await expect(page.locator(".pool-controls")).toBeVisible();
});

test("unavailable browser storage never prevents real native data", async ({ page }) => {
  await installTauriMock(page, { locale: "en" });
  await page.addInitScript(() => Object.defineProperty(window, "indexedDB", { get() { throw new Error("Synthetic storage unavailable"); } }));
  await page.goto("/");
  await expect(page.getByRole("button", { name: "Stop API", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
});
