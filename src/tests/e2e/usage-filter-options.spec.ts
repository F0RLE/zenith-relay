import { expect, test, type Page } from "../bun-playwright";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

for (const staleOutcome of ["success", "failure"] as const) {
  test(`late filter ${staleOutcome} cannot replace newer history choices`, async ({ page }) => {
    await page.clock.install();
    await installTauriMock(page, { locale: "en" });
    await page.goto("/");
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
    await page.clock.pauseAt(new Date(Date.now() + 1_000));
    await page.evaluate((staleOutcome) => {
      const target = window as unknown as {
        __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
        __RELEASE_OLD_CHOICES__?: () => void;
        __CHOICE_READS__: number;
      };
      const invoke = target.__TAURI_INTERNALS__.invoke.bind(target.__TAURI_INTERNALS__);
      target.__CHOICE_READS__ = 0;
      target.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
        const result = await invoke(command, args, options);
        if (command !== "get_local_usage_page" || !(args as { input?: { includeFilterOptions?: boolean } })?.input?.includeFilterOptions) return result;
        target.__CHOICE_READS__ += 1;
        const updated = structuredClone(result) as { filterOptions: { models: string[]; poolMembers: string[] } };
        updated.filterOptions.models = [target.__CHOICE_READS__ === 1 ? "stale-model" : "current-model"];
        if (target.__CHOICE_READS__ !== 1) return updated;
        return new Promise((resolve, reject) => {
          target.__RELEASE_OLD_CHOICES__ = () => staleOutcome === "failure"
            ? reject(new Error("Synthetic stale filter failure")) : resolve(updated);
        });
      };
    }, staleOutcome);
    const models = page.locator(".usage-model-menu > button");
    await models.click();
    await expect.poll(() => page.evaluate(() => typeof (window as unknown as { __RELEASE_OLD_CHOICES__?: () => void }).__RELEASE_OLD_CHOICES__)).toBe("function");
    await page.keyboard.press("Escape");
    await emitTauriEvent(page, "zenith-usage-recorded", null);
    await page.clock.fastForward(500);
    await models.click();
    await expect(page.getByRole("option", { name: "current-model", exact: true })).toBeVisible();
    await page.evaluate(() => (window as unknown as { __RELEASE_OLD_CHOICES__: () => void }).__RELEASE_OLD_CHOICES__());
    await page.clock.fastForward(100);
    await expect(page.getByRole("option", { name: "current-model", exact: true })).toBeVisible();
    await expect(page.getByRole("option", { name: "stale-model", exact: true })).toHaveCount(0);
    await expect(page.locator(".usage-page").getByRole("alert")).toHaveCount(0);
    await page.keyboard.press("Escape");
    await models.click();
    expect(await page.evaluate(() => (window as unknown as { __CHOICE_READS__: number }).__CHOICE_READS__)).toBe(2);
  });
}

async function expectOptions(page: Page, label: string, values: string[]) {
  await page.getByRole("button", { name: new RegExp(`^${label}:`) }).click();
  await expect.poll(() => page.getByRole("option").evaluateAll((options) =>
    options.map((option) => option.getAttribute("data-value")))).toEqual(["", ...values]);
  await page.keyboard.press("Escape");
}

for (const version of ["advanced", "missing"] as const) {
  for (const width of [1160, 390]) {
    test(`older host model/member choices use whole-period history: ${version}, ${width}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await installTauriMock(page, {
        mode: "local", locale: "ru", populated: true, usageFilterOptionsVersion: version,
        usageFilterHistory: [
          ...Array.from({ length: 51 }, () => ({
            wireApi: "responses" as const, transport: "http" as const, errorCategory: null,
            requestedModel: "requested-current", resolvedModel: "history-current",
            accountId: "historical-account", sourceId: "internal-source",
          })),
          { wireApi: "messages", transport: "websocket", errorCategory: "upstream_failure", ageDays: 2,
            requestedModel: "requested-old", resolvedModel: "history-older", accountId: null, sourceId: "historical-source" },
        ],
      });
      await page.goto("/");
      await page.getByRole("button", { name: "Использование", exact: true }).click();
      await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(50);
      await expectOptions(page, "Модель", ["history-current", "history-older"]);
      await expectOptions(page, "Участник пула", ["historical-account", "historical-source"]);
      await page.getByRole("button", { name: /^Модель:/ }).click();
      await page.getByRole("option", { name: "history-older", exact: true }).click();
      await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
      await expectOptions(page, "Модель", ["history-current", "history-older"]);
      await expectOptions(page, "Участник пула", ["historical-account", "historical-source"]);
      await page.getByRole("button", { name: "Сбросить фильтры", exact: true }).click();
      await page.getByRole("button", { name: /^Период:/ }).click();
      await page.locator('[role="option"][data-value="daily"]').click();
      await expectOptions(page, "Модель", ["history-current"]);
      await expectOptions(page, "Участник пула", ["historical-account"]);

      const reportQueries = await page.evaluate(() => (window as unknown as {
        __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: Record<string, unknown> } }>;
      }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "get_local_usage_page").map((call) => call.args.input));
      const historyQueries = reportQueries.filter((query) => query?.includeModels === true && query?.includePoolMembers === true);
      expect(historyQueries.length).toBeGreaterThan(0);
      for (const query of historyQueries) {
        expect(query).toMatchObject({ includeEvents: false });
        expect(query).not.toHaveProperty("modelQuery");
        expect(query).not.toHaveProperty("sourceOrAccountQuery");
        expect(query).not.toHaveProperty("page");
      }
      await page.getByRole("button", { name: /^Модель:/ }).click();
      await page.screenshot({ path: `output/playwright/usage-models-older-host-${version}-${width}.png` });
    });
  }
}

for (const locale of ["ru", "en"] as const) {
  test(`usage model and member selectors retain history outside the catalog: ${locale}`, async ({ page }) => {
    const labels = locale === "ru" ? {
      usage: "Использование", model: "Модель", member: "Участник пула", range: "Период",
      models: "Модели", members: "Участники пула", errors: "Ошибки", requests: "Отправки",
      filters: "Другие фильтры", clear: "Сбросить фильтры",
    } : {
      usage: "Usage", model: "Model", member: "Pool member", range: "Range",
      models: "Models", members: "Pool members", errors: "Errors", requests: "Sends",
      filters: "More filters", clear: "Clear filters",
    };
    await installTauriMock(page, {
      mode: "local", locale, populated: true,
      usageFilterHistory: [
        ...Array.from({ length: 51 }, () => ({
          wireApi: "responses" as const, transport: "http" as const, errorCategory: "upstream_failure",
          requestedModel: "history-requested", resolvedModel: "history-served",
          accountId: "removed-account-a", sourceId: "internal-source-a",
        })),
        { wireApi: "messages", transport: "websocket", errorCategory: "rate_limited", ageDays: 2,
          requestedModel: "old-requested", resolvedModel: "old-served", accountId: null, sourceId: "removed-source-b" },
      ],
    });
    await page.goto("/");
    await page.getByRole("button", { name: labels.usage, exact: true }).click();
    await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(50);
    const modelValues = ["history-requested", "history-served", "old-requested", "old-served"];
    const memberValues = ["removed-account-a", "removed-source-b"];
    await expectOptions(page, labels.model, modelValues);
    await expectOptions(page, labels.member, memberValues);

    const usageReads = () => page.evaluate(() => (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string }>;
    }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "get_local_usage_page").length);
    const before = await usageReads();
    await page.getByRole("button", { name: labels.filters, exact: true }).click();
    await expect(page.getByRole("button", { name: labels.filters, exact: true })).toHaveAttribute("aria-expanded", "true");
    expect(await usageReads()).toBe(before);

    await page.getByRole("button", { name: new RegExp(`^${labels.model}:`) }).click();
    await page.locator('[role="option"][data-value="old-requested"]').click();
    await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
    await expectOptions(page, labels.model, modelValues);
    await expectOptions(page, labels.member, memberValues);
    await page.getByRole("button", { name: new RegExp(`^${labels.member}:`) }).click();
    await page.locator('[role="option"][data-value="removed-account-a"]').click();
    await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(0);
    await expectOptions(page, labels.model, modelValues);
    await expectOptions(page, labels.member, memberValues);
    await page.getByRole("button", { name: labels.clear, exact: true }).click();

    await page.getByRole("tab", { name: labels.models, exact: true }).click();
    await expectOptions(page, labels.member, memberValues);
    await page.getByRole("tab", { name: labels.members, exact: true }).click();
    await expectOptions(page, labels.model, modelValues);
    await page.getByRole("tab", { name: labels.errors, exact: true }).click();
    await expectOptions(page, labels.model, modelValues);
    await expectOptions(page, labels.member, memberValues);
    await page.getByRole("tab", { name: labels.requests, exact: true }).click();
    await page.getByRole("button", { name: new RegExp(`^${labels.range}:`) }).click();
    await page.locator('[role="option"][data-value="daily"]').click();
    await expectOptions(page, labels.model, ["history-requested", "history-served"]);
    await expectOptions(page, labels.member, ["removed-account-a"]);
  });

  test(`usage filters use the complete period history, not page rows: ${locale}`, async ({ page }) => {
    const labels = locale === "ru" ? {
      usage: "Использование", filters: "Другие фильтры", protocol: "Протокол",
      transport: "Транспорт приложения", category: "Категория ошибки", range: "Период",
      clear: "Сбросить фильтры", next: "Продолжить",
    } : {
      usage: "Usage", filters: "More filters", protocol: "Protocol",
      transport: "Client transport", category: "Error category", range: "Range",
      clear: "Clear filters", next: "Continue",
    };
    await installTauriMock(page, {
      mode: "local", locale, populated: true,
      usageFilterHistory: [
        ...Array.from({ length: 51 }, () => ({
          wireApi: "responses" as const, transport: "http" as const, errorCategory: "upstream_failure",
        })),
        { wireApi: "messages", transport: "websocket", errorCategory: "rate_limited" },
        { wireApi: "gemini", transport: "http", errorCategory: "upstream_transport", ageDays: 2 },
      ],
    });
    await page.goto("/");
    await page.getByRole("button", { name: labels.usage, exact: true }).click();
    await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(50);
    await page.getByRole("button", { name: labels.filters, exact: true }).click();

    const expectFullHistory = async () => {
      await expectOptions(page, labels.protocol, ["gemini", "messages", "responses"]);
      await expectOptions(page, labels.transport, ["http", "websocket"]);
      await expectOptions(page, labels.category, ["rate_limited", "upstream_failure", "upstream_transport"]);
    };
    await expectFullHistory();

    await page.getByRole("button", { name: new RegExp(`^${labels.category}:`) }).click();
    await page.locator('[role="option"][data-value="rate_limited"]').click();
    await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
    await expectFullHistory();
    await page.getByRole("button", { name: new RegExp(`^${labels.transport}:`) }).click();
    await page.locator('[role="option"][data-value="http"]').click();
    await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(0);
    await expectFullHistory();

    await page.getByRole("button", { name: labels.clear, exact: true }).click();
    await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(50);
    await page.locator(".usage-pagination").getByRole("button", { name: labels.next, exact: true }).click();
    await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(3);
    await expectFullHistory();

    await page.getByRole("button", { name: new RegExp(`^${labels.range}:`) }).click();
    await page.locator('[role="option"][data-value="daily"]').click();
    await expect(page.locator('.usage-pagination [aria-current="page"]')).toHaveText("1");
    await expectOptions(page, labels.protocol, ["messages", "responses"]);
    await expectOptions(page, labels.transport, ["http", "websocket"]);
    await expectOptions(page, labels.category, ["rate_limited", "upstream_failure"]);
    await page.getByRole("button", { name: new RegExp(`^${labels.range}:`) }).click();
    await page.locator('[role="option"][data-value="all"]').click();
    await expectFullHistory();
  });
}
