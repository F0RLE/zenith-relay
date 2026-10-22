import { expect, test, type Page } from "../bun-playwright";
import type { RequestTimezoneUpdate } from "../../src/features/relay/api/types";
import { installTauriMock } from "./tauri-mock";

const NEUTRAL_TIMEZONE_CATALOG = [
  "Etc/GMT+12", "Etc/GMT+11", "Etc/GMT+10", "Etc/GMT+9", "Etc/GMT+8", "Etc/GMT+7",
  "Etc/GMT+6", "Etc/GMT+5", "Etc/GMT+4", "Etc/GMT+3", "Etc/GMT+2", "Etc/GMT+1",
  "UTC",
  "Etc/GMT-1", "Etc/GMT-2", "Etc/GMT-3", "Etc/GMT-4", "Etc/GMT-5", "Etc/GMT-6",
  "Etc/GMT-7", "Etc/GMT-8", "Etc/GMT-9", "Etc/GMT-10", "Etc/GMT-11", "Etc/GMT-12",
  "America/New_York",
] as const;

async function openAccountTimezone(page: Page, russian = false) {
  await page.getByRole("button", { name: russian ? "Подключения" : "Connections", exact: true }).click();
  const card = page.locator(".account-card").filter({ hasText: "Personal Plus" });
  await card.locator(".account-row-menu summary").click();
  await page.getByRole("menuitem", { name: russian ? "Часовой пояс аккаунта" : "Account timezone", exact: true }).click();
  return page.getByRole("dialog", { name: russian ? "Часовой пояс аккаунта" : "Account timezone", exact: true });
}

async function timezoneUpdates(page: Page) {
  return page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__;
    return calls.filter((call) => call.command === "set_local_request_timezone")
      .map((call) => call.args["input"] as RequestTimezoneUpdate);
  });
}

for (const mode of ["local"] as const) {
  test(`${mode} account menu saves and resets only the selected account's timezone`, async ({ page }, testInfo) => {
    const initial = {
      enabled: true,
      accountTimezones: { account_synthetic_2: "Etc/GMT+8" },
      accountTimezoneBindings: {}, accountTimezoneSources: {},
    };
    await installTauriMock(page, { mode, populated: true, accountCount: 2, requestTimezone: initial });
    await page.goto("/");
    const dialog = await openAccountTimezone(page);
    await expect(dialog.getByRole("radio", { name: /America\/New_York/ })).toBeChecked();
    await expect(dialog.getByRole("radio", { name: "Auto", exact: true })).toBeChecked();
    await expect(dialog).toContainText("The default is the account setting.");
    await dialog.locator("label.account-timezone-option", { hasText: "Etc/GMT-3" }).click();
    await expect(dialog.getByRole("radio", { name: /Etc\/GMT-3/ })).toBeChecked();
    await dialog.getByRole("button", { name: "Save", exact: true }).click();
    await expect(dialog).not.toBeVisible();
    expect((await timezoneUpdates(page)).at(-1)).toEqual({
      expectedPolicy: initial,
      policy: {
        enabled: true,
        accountTimezones: { ...initial.accountTimezones, account_synthetic: "Etc/GMT-3" },
        accountTimezoneBindings: {}, accountTimezoneSources: {},
      },
    });
    const otherCard = page.locator(".account-card").filter({ hasText: "Business Workspace" });
    await otherCard.locator(".account-row-menu summary").click();
    await page.getByRole("menuitem", { name: "Account timezone", exact: true }).click();
    await expect(dialog.getByRole("radio", { name: /Etc\/GMT\+8/ })).toBeChecked();
    await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
    await openAccountTimezone(page);
    await expect(dialog.getByRole("radio", { name: /Etc\/GMT-3/ })).toBeChecked();
    await page.screenshot({ path: testInfo.outputPath("account-timezone-selected.png") });
    await dialog.locator("label.account-timezone-option", { hasText: "America/New_York" }).click();
    await expect(dialog.getByRole("radio", { name: /America\/New_York/ })).toBeChecked();
    await dialog.getByRole("button", { name: "Save", exact: true }).click();
    await expect(dialog).not.toBeVisible();
    expect((await timezoneUpdates(page)).at(-1)?.policy).toEqual(initial);
  });
}

for (const mode of ["local"] as const) {
  test(`${mode} account timezone can override and then inherit the global setting`, async ({ page }) => {
    await installTauriMock(page, { mode, populated: true, requestTimezone: {
      enabled: false,
      accountTimezones: {},
      accountTimezoneBindings: {}, accountTimezoneSources: {},
    } });
    await page.goto("/");
    const dialog = await openAccountTimezone(page);
    const useHere = dialog.getByRole("radio", { name: "Set", exact: true });
    await expect(dialog.getByRole("radio", { name: "Auto", exact: true })).toBeChecked();
    await expect(dialog.getByRole("radio", { name: /America\/New_York/ })).toHaveCount(0);
    await useHere.click();
    await expect(useHere).toBeChecked();
    await expect(dialog.getByRole("radio", { name: /America\/New_York/ })).toBeChecked();
    await dialog.getByRole("button", { name: "Save", exact: true }).click();
    expect((await timezoneUpdates(page)).at(-1)?.policy).toEqual({
      enabled: false,
      accountTimezones: {},
      accountTimezoneBindings: { account_synthetic: true }, accountTimezoneSources: {},
    });

    await openAccountTimezone(page);
    await expect(dialog.getByRole("radio", { name: "Set", exact: true })).toBeChecked();
    await dialog.getByRole("radio", { name: "Auto", exact: true }).click();
    await expect(dialog).toContainText("The default is the assigned proxy's timezone.");
    await dialog.getByRole("button", { name: "Save", exact: true }).click();
    expect((await timezoneUpdates(page)).at(-1)?.policy).toEqual({
      enabled: false,
      accountTimezones: {},
      accountTimezoneBindings: {}, accountTimezoneSources: {},
    });
  });
}

test("account selection keeps binding off and cancel does not save a draft", async ({ page }) => {
  await installTauriMock(page, { populated: true });
  await page.goto("/");
  const dialog = await openAccountTimezone(page);
  await expect(dialog).toContainText("The default is the assigned proxy's timezone.");
  await dialog.getByRole("radio", { name: "Set", exact: true }).click();
  await dialog.locator("label.account-timezone-option", { hasText: "Etc/GMT-3" }).click();
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  expect(await timezoneUpdates(page)).toEqual([]);
  await openAccountTimezone(page);
  await expect(dialog.getByRole("radio", { name: "Auto", exact: true })).toBeChecked();
  await dialog.getByRole("radio", { name: "Set", exact: true }).click();
  await dialog.locator("label.account-timezone-option", { hasText: "Etc/GMT-3" }).click();
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await expect(dialog).not.toBeVisible();
  expect((await timezoneUpdates(page)).at(-1)?.policy).toEqual({
    enabled: false,
    accountTimezones: { account_synthetic: "Etc/GMT-3" },
    accountTimezoneBindings: { account_synthetic: true }, accountTimezoneSources: {},
  });
});

test("empty search and keyboard selection keep the dialog usable", async ({ page }) => {
  await installTauriMock(page, { populated: true, timezoneCatalog: ["America/New_York", "Etc/GMT-3"] });
  await page.goto("/");
  const dialog = await openAccountTimezone(page);
  await dialog.getByRole("radio", { name: "Set", exact: true }).click();
  await expect(dialog.getByRole("radio", { name: /Etc\/GMT-3|America\/New_York/ })).toHaveCount(2);
  const search = dialog.getByRole("searchbox", { name: "City, timezone or UTC offset" });
  await search.fill("Unknown city");
  await expect(dialog).toContainText("No timezones found.");
  await search.fill("GMT-3");
  const zone = dialog.getByRole("radio", { name: /Etc\/GMT-3/ });
  await expect(zone).toHaveCount(1);
  await zone.focus();
  await page.keyboard.press("Space");
  await expect(zone).toBeChecked();
  await page.keyboard.press("Escape");
  await expect(dialog).not.toBeVisible();
  expect(await timezoneUpdates(page)).toEqual([]);
});


test("missing proxy preview does not report an available store as broken", async ({ page }) => {
  await installTauriMock(page, { populated: true });
  await page.goto("/");
  const dialog = await openAccountTimezone(page);
  await expect(dialog.getByRole("status")).toContainText("The assigned proxy's timezone is unavailable for preview.");
  await expect(dialog).not.toContainText("Proxy storage is unavailable");
  await expect(dialog.getByRole("radio", { name: "Auto", exact: true })).toBeChecked();
  expect(await timezoneUpdates(page)).toEqual([]);
});

test("catalog failures can be retried without losing a saved choice", async ({ page }) => {
  await installTauriMock(page, {
    populated: true,
    requestTimezone: {
      enabled: true,
      accountTimezones: { account_synthetic: "Etc/GMT-3" },
      accountTimezoneBindings: {}, accountTimezoneSources: {},
    },
  });
  await page.goto("/");
  await page.evaluate(() => {
    const tauri = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = tauri.invoke.bind(tauri);
    let failed = false;
    tauri.invoke = async (command, args) => {
      if (command === "get_request_timezone_catalog" && !failed) { failed = true; throw { code: "unavailable" }; }
      return invoke(command, args);
    };
  });
  const dialog = await openAccountTimezone(page);
  await expect(dialog.getByRole("alert")).toContainText("The timezone catalog is unavailable.");
  // A catalog read failure must not prevent retaining an already saved zone.
  await expect(dialog.getByRole("button", { name: "Save", exact: true })).toBeEnabled();
  expect(await timezoneUpdates(page)).toEqual([]);
  await dialog.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(dialog.getByRole("radio", { name: /Etc\/GMT-3/ })).toBeChecked();
  await expect(dialog.getByRole("button", { name: "Save", exact: true })).toBeEnabled();
});

test("stale account drafts fail without overwriting a concurrent policy update", async ({ page }) => {
  await installTauriMock(page, { populated: true, accountCount: 2 });
  await page.goto("/");
  const dialog = await openAccountTimezone(page);
  await dialog.getByRole("radio", { name: "Set", exact: true }).click();
  await dialog.locator("label.account-timezone-option", { hasText: "Etc/GMT-3" }).click();
  await page.evaluate(async () => {
    const tauri = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args: Record<string, unknown>) => Promise<unknown> } }).__TAURI_INTERNALS__;
    await tauri.invoke("set_local_request_timezone", { input: {
      expectedPolicy: { enabled: false, accountTimezones: {}, accountTimezoneBindings: {}, accountTimezoneSources: {} },
      policy: {
        enabled: true,
        accountTimezones: { account_synthetic_2: "Etc/GMT+8" },
        accountTimezoneBindings: {}, accountTimezoneSources: {},
      },
    } });
  });
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await expect(dialog.locator(".account-timezone-error")).toBeVisible();
  await expect(dialog).toBeVisible();
  const savedPolicy = await page.evaluate(async () => {
    const tauri = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<{ gateway: { requestTimezone: unknown } }> } }).__TAURI_INTERNALS__;
    return (await tauri.invoke("get_local_runtime_state")).gateway.requestTimezone;
  });
  expect(savedPolicy).toEqual({
    enabled: true,
    accountTimezones: { account_synthetic_2: "Etc/GMT+8" },
    accountTimezoneBindings: {}, accountTimezoneSources: {},
  });
});

for (const theme of ["dark", "light"] as const) {
  test(`Russian account timezone picker fits a narrow ${theme} window with a full catalog`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await installTauriMock(page, { populated: true, locale: "ru", theme, timezoneCatalog: [...NEUTRAL_TIMEZONE_CATALOG], requestTimezone: {
      enabled: true,
      accountTimezones: {},
      accountTimezoneBindings: {}, accountTimezoneSources: {},
    } });
    await page.goto("/");
    const dialog = await openAccountTimezone(page, true);
    await expect(dialog.getByRole("radio").first()).toBeVisible();
    await expect(dialog.getByRole("radio", { name: /America\/New_York/ })).toBeChecked();
    await expect(dialog.getByRole("button", { name: "Сохранить", exact: true })).toBeInViewport();
    await page.screenshot({ path: testInfo.outputPath(`account-timezone-default-ru-${theme}.png`) });
    await dialog.locator("label.account-timezone-option", { hasText: "Etc/GMT-3" }).click();
    const bounds = await dialog.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      const body = element.querySelector(".relay-dialog-body")!;
      const list = element.querySelector(".account-timezone-list")!;
      return {
        fits: rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight,
        bodyFits: body.scrollWidth <= body.clientWidth + 1,
        listFits: list.scrollWidth <= list.clientWidth + 1,
        listScrolls: list.scrollHeight > list.clientHeight,
      };
    });
    expect(bounds).toEqual({ fits: true, bodyFits: true, listFits: true, listScrolls: true });
    await expect(dialog.getByRole("button", { name: "Сохранить", exact: true })).toBeInViewport();
    await page.screenshot({ path: testInfo.outputPath(`account-timezone-ru-${theme}-narrow.png`) });
  });
}
