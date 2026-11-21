import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const locale of ["en", "ru"] as const) {
  for (const width of [1160, 360]) {
    test(`proxy cards preserve their header and action strip with four middle metrics: ${locale} ${width}`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width, height: 900 });
      await installTauriMock(page, { locale, theme: locale === "ru" ? "dark" : "light", populated: true, proxyCount: 2, proxySpeedDelayMs: 300 });
      await page.goto("/");
      await page.getByRole("button", { name: locale === "ru" ? "Подключения" : "Connections", exact: true }).click();
      await page.getByRole("tab", { name: locale === "ru" ? "Прокси" : "Proxies", exact: true }).click();
      const row = page.locator(".proxy-storage-row").first();
      const actions = row.locator(".account-card-footer");
      const buttons = actions.getByRole("button");
      const speed = row.getByRole("button", { name: locale === "ru" ? "Тест скорости" : "Test speed", exact: true });
      await expect(buttons).toHaveCount(3);
      await expect(buttons.nth(0)).toHaveAccessibleName(locale === "ru" ? "Проверить прокси" : "Test proxy");
      expect(await buttons.nth(1).getAttribute("class")).toContain("proxy-speed-test");
      await expect(buttons.nth(2)).toHaveAccessibleName(locale === "ru" ? "Управлять привязанными аккаунтами" : "Manage assigned accounts");
      await expect(row.locator(".account-kind-icon")).toBeVisible();
      await expect(row.locator(".account-select-button")).toBeVisible();
      for (const button of await buttons.all()) {
        await expect(button).toHaveText("");
      }
      const initialWidths = await buttons.evaluateAll((elements) => elements.map((element) => element.getBoundingClientRect().width));
      expect(Math.max(...initialWidths) - Math.min(...initialWidths)).toBeLessThanOrEqual(1);
      await expect(row.locator(".proxy-speed-value")).toHaveText("—");
      await expect(row.getByText("http://proxy-1.example.test:10000", { exact: true })).toHaveCount(1);
      await expect(row).not.toContainText("203.0.113.42");
      await row.locator(".account-row-menu summary").click();
      await page.getByRole("menuitem", { name: locale === "ru" ? "Название прокси" : "Rename proxy", exact: true }).click();
      await page.getByRole("textbox", { name: locale === "ru" ? "Название" : "Proxy name", exact: true }).fill(locale === "ru" ? "Рабочий прокси" : "Work proxy");
      await page.getByRole("button", { name: locale === "ru" ? "Сохранить" : "Save", exact: true }).click();
      await expect(page.getByRole("dialog")).toHaveCount(0);
      await expect(row.locator(".account-identity > strong")).toHaveText(locale === "ru" ? "Рабочий прокси" : "Work proxy");
      await expect(row.getByText("http://proxy-1.example.test:10000", { exact: true })).toHaveCount(1);
      const measurements = () => page.evaluate(() => (window as unknown as {
        __TAURI_TEST_INVOKES__: Array<{ command: string; args: unknown }>;
      }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "test_local_stored_proxy_speed"));
      expect(await measurements()).toHaveLength(0);
      await speed.hover();
      await expect(page.getByRole("tooltip")).toHaveText(locale === "ru" ? "Тест скорости" : "Test speed");
      const before = await actions.boundingBox();
      await speed.click();
      await expect(speed).toHaveAttribute("aria-busy", "true");
      await speed.dispatchEvent("click");
      await expect(speed).toHaveAttribute("data-state", "success");
      expect(await measurements()).toEqual([expect.objectContaining({ args: { proxyId: "proxy_synthetic_1", onStarted: expect.stringMatching(/^__CHANNEL__:/) } })]);
      await speed.hover();
      await expect(page.getByRole("tooltip")).toHaveText(locale === "ru" ? "25,4 Мбит/с" : "25.4 Mbps");
      await expect(row.locator(".proxy-speed-value")).toHaveText(locale === "ru" ? "25,4 Мбит/с" : "25.4 Mbps");
      const accounts = row.locator(".proxy-card-accounts");
      await expect(accounts).toHaveText(/^\d+$/);
      await accounts.hover();
      await expect(page.getByRole("tooltip")).toHaveText(locale === "ru" ? "Аккаунты" : "Accounts");
      await page.mouse.move(0, 0);
      const card = (await row.boundingBox())!;
      const header = (await row.locator(".account-card-main").boundingBox())!;
      const middle = (await row.locator(".proxy-card-details").boundingBox())!;
      const location = (await row.locator(".proxy-card-location").boundingBox())!;
      const ping = (await row.locator(".proxy-card-latency").boundingBox())!;
      const speedBox = (await row.locator(".proxy-speed-value").boundingBox())!;
      const accountsBox = (await accounts.boundingBox())!;
      const cells = row.locator(".proxy-card-location, .proxy-card-latency, .proxy-speed-value, .proxy-card-accounts");
      const cellStyles = await cells.evaluateAll((elements) => elements.map((element) => {
        const style = getComputedStyle(element);
        const box = element.getBoundingClientRect();
        return { width: box.width, textAlign: style.textAlign, justify: style.justifyContent, top: style.borderTopWidth, left: style.borderLeftWidth };
      }));
      expect(Math.max(...cellStyles.map((cell) => cell.width)) - Math.min(...cellStyles.map((cell) => cell.width))).toBeLessThanOrEqual(1);
      expect(cellStyles.every((cell) => cell.textAlign === "center" && cell.justify === "center")).toBe(true);
      expect(cellStyles.map((cell) => [cell.top, cell.left])).toEqual([["0px", "0px"], ["0px", "1px"], ["1px", "0px"], ["1px", "1px"]]);
      expect(Math.abs(middle.x + middle.width / 2 - card.x - card.width / 2)).toBeLessThanOrEqual(1);
      expect(location.y).toBeGreaterThanOrEqual(header.y + header.height);
      expect(ping.y).toBeGreaterThanOrEqual(header.y + header.height);
      expect(location.x).toBeLessThan(ping.x);
      expect(speedBox.y).toBeGreaterThan(location.y);
      expect(accountsBox.y).toBeGreaterThan(ping.y);
      expect(speedBox.x).toBeLessThan(accountsBox.x);
      expect(Math.abs(speedBox.y + speedBox.height / 2 - accountsBox.y - accountsBox.height / 2)).toBeLessThanOrEqual(1);
      expect((await actions.boundingBox())!.y).toBeGreaterThanOrEqual(middle.y + middle.height);
      expect((await actions.boundingBox())!.height).toBe(before!.height);
      expect(await row.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
      await page.screenshot({ path: testInfo.outputPath(`proxy-speed-${locale}-${width}.png`) });
      await buttons.nth(2).click();
      await expect(page.getByRole("dialog")).toBeVisible();
    });
  }
}

test("a failed speed test is visible on the card and can be retried", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", populated: true, proxyCount: 1, proxySpeedError: "proxy_check_timeout" });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.getByRole("tab", { name: "Прокси", exact: true }).click();
  const row = page.locator(".proxy-storage-row").first();
  const speed = row.getByRole("button", { name: "Тест скорости", exact: true });
  await speed.click();
  await expect(speed).toHaveAttribute("data-state", "error");
  await expect(row.locator(".proxy-speed-value")).not.toBeEmpty();
  await speed.focus();
  await expect(page.getByRole("tooltip")).toContainText("12 секунд");
  await expect(row).not.toContainText("12 секунд");
  await expect(row).not.toContainText("0 Мбит/с");
  await speed.click();
  await expect(speed).toHaveAttribute("data-state", "error");
  const count = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string }>;
  }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "test_local_stored_proxy_speed").length);
  expect(count).toBe(2);
});

for (const locale of ["en", "ru"] as const) {
  test(`speed tests queue across cards in click order without overlapping: ${locale}`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width: 1280, height: 900 });
    await installTauriMock(page, { locale, theme: "dark", populated: true, proxyCount: 3, proxySpeedDelayMs: 1200 });
    await page.goto("/");
    await page.getByRole("button", { name: locale === "ru" ? "Подключения" : "Connections", exact: true }).click();
    await page.getByRole("tab", { name: locale === "ru" ? "Прокси" : "Proxies", exact: true }).click();
    const cards = page.locator(".proxy-storage-row");
    const controls = cards.locator(".proxy-speed-test");
    await controls.nth(0).click();
    await expect(controls.nth(0)).toHaveAttribute("data-state", "pending");
    await controls.nth(1).click();
    await controls.nth(2).click();
    await expect(controls.nth(1)).toHaveAttribute("data-state", "queued");
    await expect(controls.nth(2)).toHaveAttribute("data-state", "queued");
    await expect(cards.nth(1).locator(".proxy-speed-value")).toHaveText(locale === "ru" ? "В очереди…" : "Queued…");
    await controls.nth(1).dispatchEvent("click");
    await page.screenshot({ path: testInfo.outputPath(`proxy-speed-queue-${locale}.png`) });
    await expect(controls.nth(0)).toHaveAttribute("data-state", "success");
    await expect(controls.nth(1)).toHaveAttribute("data-state", "pending");
    await expect(controls.nth(2)).toHaveAttribute("data-state", "queued");
    await expect(controls.nth(1)).toHaveAttribute("data-state", "success");
    await expect(controls.nth(2)).toHaveAttribute("data-state", "pending");
    await expect(controls.nth(2)).toHaveAttribute("data-state", "success");
    const ids = await page.evaluate(() => (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string; args: { proxyId: string } }>;
    }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "test_local_stored_proxy_speed").map((call) => call.args.proxyId));
    expect(ids).toEqual(["proxy_synthetic_1", "proxy_synthetic_2", "proxy_synthetic_3"]);
    await expect(cards.locator('.proxy-speed-value[data-state="error"]')).toHaveCount(0);
  });
}

for (const locale of ["ru", "en"] as const) {
  test(`proxy names are saved through IPC, searchable and clearable: ${locale}`, async ({ page }) => {
    await installTauriMock(page, { locale, populated: true, proxyCount: 1 });
    await page.goto("/");
    const open = async () => {
      await page.getByRole("button", { name: locale === "ru" ? "Подключения" : "Connections", exact: true }).click();
      await page.getByRole("tab", { name: locale === "ru" ? "Прокси" : "Proxies", exact: true }).click();
    };
    await open();
    const row = page.locator(".proxy-storage-row").first();
    const save = async (name: string) => {
      await row.locator(".account-row-menu summary").click();
      await page.getByRole("menuitem", { name: locale === "ru" ? "Название прокси" : "Rename proxy", exact: true }).click();
      await page.getByRole("textbox", { name: locale === "ru" ? "Название" : "Proxy name", exact: true }).fill(name);
      await page.getByRole("button", { name: locale === "ru" ? "Сохранить" : "Save", exact: true }).click();
      await expect(page.getByRole("dialog")).toHaveCount(0);
    };
    await save("  Work proxy  ");
    await expect(row.locator(".account-identity > strong")).toHaveText("Work proxy");
    const pool = await page.evaluate(async () => (window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string) => Promise<{ entries: Array<{ label?: string }> }> };
    }).__TAURI_INTERNALS__.invoke("get_local_proxy_pool"));
    expect(pool.entries[0]?.label).toBe("Work proxy");
    await page.locator(".proxy-storage-toolbar input:not([type=checkbox])").fill("Work proxy");
    await expect(row).toBeVisible();
    await page.locator(".proxy-storage-toolbar input:not([type=checkbox])").fill("");
    await page.getByRole("button", { name: locale === "ru" ? "Настройки" : "Settings", exact: true }).click();
    await open();
    await expect(row.locator(".account-identity > strong")).toHaveText("Work proxy");
    await save("");
    await expect(row.locator(".account-identity > strong")).toHaveText("http://proxy-1.example.test:10000");
    await expect(row.locator(".proxy-card-endpoint")).toHaveCount(0);
  });
}
