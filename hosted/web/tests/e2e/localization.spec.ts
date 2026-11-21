import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { formatMessage, locales, localeStorageKey } from "../../src/i18n/catalog";
import type { Locale } from "../../src/i18n/catalog";
import type { MessageKey, Messages } from "../../src/i18n/types";

// No inherited Russian regression state: this suite exercises fresh profiles.
test.use({ storageState: { cookies: [], origins: [] }, locale: "ru-RU" });

async function choose(page: Page, locale: Locale) {
  const trigger = page.locator(".language-roller-trigger");
  await trigger.click();
  for (let i = 0; i < locales.length; i++) {
    if (await page.locator('.language-roller-list [data-active="true"] .language-flag').getAttribute("data-locale") === locale) break;
    await page.locator('.language-roller-step[data-direction="next"]').click();
  }
  await page.locator('.language-roller-list [data-active="true"]').click();
  await expect(page.locator("html")).toHaveAttribute("lang", locale);
  await expect(page.locator(".hosted-app")).toHaveAttribute("lang", locale);
}

async function fits(page: Page) {
  await page.evaluate(() => document.fonts.ready);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  expect(await page.locator(".relay-content").evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
  for (const popup of await page.locator('.date-calendar, [role="dialog"]:visible').all()) {
    expect(await popup.evaluate(el => { const box = el.getBoundingClientRect(); return box.left >= 0 && box.right <= innerWidth && box.top >= 0 && box.bottom <= innerHeight + 1; })).toBe(true);
  }
}

test("a fresh profile is English even on a Russian browser; invalid preferences fall back to English", async ({ page }) => {
  await page.goto("/app/overview");
  await expect(page.getByRole("heading", { name: "Overview", exact: true })).toBeVisible();
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await expect(page.locator(".hosted-app")).toHaveAttribute("lang", "en");
  await page.evaluate(key => localStorage.setItem(key, "unsupported"), localeStorageKey);
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await page.goto("/");
  await expect(page.getByRole("heading", { level: 1 })).toContainText("AI models.");
});

for (const { code } of locales) test(`${code}: every Hosted route, request details and notifications are localized`, async ({ page }, testInfo) => {
  const messages: Messages = JSON.parse(readFileSync(new URL(`../../src/i18n/messages/${code}.json`, import.meta.url), "utf8"));
  const t = (key: MessageKey) => formatMessage(messages, key);
  await page.goto("/app/overview");
  await choose(page, code);
  const routes = [
    ["/app/overview", "nav.overview"], ["/app/usage", "nav.usage"], ["/app/models", "modelCatalog.title"], ["/app/keys", "nav.keys"], ["/app/billing", "nav.balance"], ["/app/status", "common.status"],
    ["/admin/overview", "nav.overview"], ["/admin/connections", "nav.connections"], ["/admin/pools", "nav.pool"], ["/admin/models", "modelCatalog.title"], ["/admin/usage", "nav.usage"],
  ] as const;
  for (const [route, heading] of routes) {
    await page.goto(route);
    await expect(page.getByRole("heading", { name: t(heading), exact: true })).toBeVisible();
    await expect(page.locator("html")).toHaveAttribute("lang", code);
    const text = await page.locator("#hosted-content").innerText();
    if (code !== "ru") expect(text).not.toMatch(/\p{Script=Cyrillic}/u);
    if (code !== "zh-CN") expect(text).not.toMatch(/\p{Script=Han}/u);
    expect(text).not.toMatch(/\{(?:count|name|label|amount|value|period)\}/);
    await fits(page);
    if (route === "/admin/connections") await page.screenshot({ path: testInfo.outputPath(`connections-${code}.png`) });
  }
  await page.locator(".request-link").first().click();
  const details = page.getByRole("dialog", { name: t("request.requestDetails") });
  await expect(details.getByText(t("request.requestInProgress"), { exact: true })).toBeVisible();
  await details.locator(".request-technical-details > summary").click();
  await expect(details.getByText(t("request.serviceTier"), { exact: true })).toBeVisible();
  await fits(page);
  await page.screenshot({ path: testInfo.outputPath(`request-${code}.png`) });
  await details.getByRole("button", { name: t("common.close"), exact: true }).click();
  await page.locator(".notification-button").click();
  await expect(page.getByRole("heading", { name: t("notifications.accountingIsStillBeingFinalized"), exact: true })).toBeVisible();
  await fits(page);
});

test("switching locale preserves typed key policy, dates, confirmations and filter selections", async ({ page }, testInfo) => {
  await page.goto("/app/keys");
  await page.getByRole("button", { name: "Create key", exact: true }).click();
  await page.getByLabel("Key name", { exact: true }).fill("My проект 中文");
  await page.getByLabel("Daily limit, USD", { exact: true }).fill("27");
  await page.getByLabel("Valid until", { exact: true }).fill("02/29/2028");
  await choose(page, "ru");
  await expect(page.getByLabel("Название", { exact: true })).toHaveValue("My проект 中文");
  await expect(page.getByLabel("Дневной лимит, USD", { exact: true })).toHaveValue("27");
  await expect(page.getByLabel("Действует до", { exact: true })).toHaveValue("29.02.2028");
  await choose(page, "zh-CN");
  await expect(page.getByLabel("有效期至", { exact: true })).toHaveValue("2028/02/29");
  await page.getByRole("button", { name: "打开日历：有效期至", exact: true }).click();
  await fits(page);
  await page.screenshot({ path: testInfo.outputPath("calendar-zh-CN.png") });
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "创建密钥", exact: true }).last().click();
  await page.getByRole("button", { name: "完成", exact: true }).click();
  const row = page.getByRole("row", { name: "My проект 中文", exact: true });
  await expect(row).toContainText("2028/02/29");
  await row.getByRole("button", { name: "重新签发", exact: true }).click();
  await expect(row.getByRole("button", { name: "确认重新签发", exact: true })).toBeVisible();
  await choose(page, "en");
  // Leaving the row deliberately disarms destructive confirmations.
  await expect(row).toHaveAttribute("data-revoked", "false");
  await row.getByRole("button", { name: "Reissue", exact: true }).click();
  await expect(row.getByRole("button", { name: "Yes, reissue", exact: true })).toBeVisible();
  await expect(row).toContainText("02/29/2028");
  await row.getByRole("button", { name: "Cancel", exact: true }).click();
  await page.goto("/app/usage");
  await page.getByRole("combobox", { name: "Model", exact: true }).click();
  await page.getByRole("option", { name: "GPT", exact: true }).click();
  await choose(page, "ru");
  await expect(page.getByRole("combobox", { name: "Модель", exact: true })).toHaveText("GPT");
});

test("locale syncs across tabs without losing a draft, and switching works when storage is denied", async ({ page, context }) => {
  await page.goto("/app/keys");
  await page.getByRole("button", { name: "Create key", exact: true }).click();
  await page.getByLabel("Key name", { exact: true }).fill("Keep this draft");
  const other = await context.newPage();
  try {
    await other.goto("/admin/overview");
    await choose(other, "zh-CN");
    await expect(page.locator("html")).toHaveAttribute("lang", "zh-CN");
    await expect(page.getByLabel("名称", { exact: true })).toHaveValue("Keep this draft");
  } finally { await other.close(); }
  await page.addInitScript(() => { Object.defineProperty(window, "localStorage", { get() { throw new DOMException("Denied", "SecurityError"); } }); });
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await choose(page, "ru");
  await expect(page.getByRole("heading", { name: "API-ключи", exact: true })).toBeVisible();
});

test("only selected dictionaries load, failed loads preserve drafts, and retry and cache work", async ({ page }) => {
  const requests: string[] = [];
  page.on("request", request => { if (/\/messages\/(ru|zh-CN)\.json/.test(request.url())) requests.push(request.url()); });
  let failOnce = true;
  await page.route(/\/messages\/ru\.json/, async route => {
    if (failOnce) { failOnce = false; await route.abort(); }
    else await route.continue();
  });
  await page.goto("/app/keys");
  await page.getByRole("button", { name: "Create key", exact: true }).click();
  await page.getByLabel("Key name", { exact: true }).fill("Keep offline draft");
  expect(requests).toEqual([]);
  await page.evaluate(key => window.dispatchEvent(new StorageEvent("storage", { key, newValue: "ru" })), localeStorageKey);
  await expect(page.getByRole("alert")).toContainText("Could not load this language");
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await expect(page.getByLabel("Key name", { exact: true })).toHaveValue("Keep offline draft");
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(page.locator("html")).toHaveAttribute("lang", "ru");
  await expect(page.getByLabel("Название", { exact: true })).toHaveValue("Keep offline draft");
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(requests.length).toBe(2);
  expect(requests.every(url => url.includes("/ru.json"))).toBe(true);
  await choose(page, "en");
  await choose(page, "ru");
  expect(requests.length).toBe(2);
  await choose(page, "zh-CN");
  expect(requests.filter(url => url.includes("/zh-CN.json"))).toHaveLength(1);
});

test("a late dictionary response cannot override a newer language choice", async ({ page }) => {
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  await page.route(/\/messages\/ru\.json/, async route => { await gate; await route.continue(); });
  await page.goto("/app/keys");
  await expect(page.getByRole("heading", { name: "API keys", exact: true })).toBeVisible();
  const requested = page.waitForRequest(/\/messages\/ru\.json/);
  await page.evaluate(key => window.dispatchEvent(new StorageEvent("storage", { key, newValue: "ru" })), localeStorageKey);
  await requested;
  try { await choose(page, "zh-CN"); } finally { release(); }
  // Await this exact resource, then a paint: no time-based race assertion.
  await page.evaluate(async () => {
    const modulePath = "/src/i18n/catalog.ts";
    const { loadLocale } = await import(modulePath);
    await loadLocale("ru");
    await new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
  });
  await expect(page.locator("html")).toHaveAttribute("lang", "zh-CN");
  await expect(page.getByRole("heading", { name: "API 密钥", exact: true })).toBeVisible();
  expect(await page.evaluate(key => localStorage.getItem(key), localeStorageKey)).toBe("zh-CN");
});
