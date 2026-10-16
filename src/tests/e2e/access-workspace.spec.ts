import { expect, test } from "../bun-playwright";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

test("Access unifies pool API and applications with keyboard navigation", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: true, gatewayRunning: false });
  await page.goto("/");
  const navigation = page.getByRole("navigation", { name: "Main navigation" });
  await expect(navigation.getByRole("button")).toHaveText(["Overview", "Connections", "Pool", "Access", "Usage"]);
  await expect(page.getByRole("navigation", { name: "Maintenance", exact: true }).getByRole("button")).toHaveText(["Recovery", "Settings"]);
  await navigation.getByRole("button", { name: "Access", exact: true }).click();
  await page.getByRole("button", { name: "Start API", exact: true }).click();
  await expect(page.getByRole("button", { name: "Stop API", exact: true })).toBeVisible();
  const apiTab = page.getByRole("tab", { name: "Local API", exact: true });
  await apiTab.focus();
  await apiTab.press("ArrowRight");
  const chatgpt = page.getByRole("tab", { name: "ChatGPT", exact: true });
  await expect(chatgpt).toHaveAttribute("aria-selected", "true");
  const picker = page.locator(".integration-picker");
  await expect(picker.getByRole("tab")).toHaveText(["ChatGPT", "Claude Desktop", "OpenCode"]);
  await expect(picker.getByRole("tablist")).toHaveAttribute("aria-orientation", "vertical");
  await expect(chatgpt.locator("img")).toHaveAttribute("src", "/icons/chatgpt.svg");
  await expect(picker.getByRole("tab", { name: "OpenCode", exact: true }).locator("img")).toHaveAttribute("src", "/icons/opencode.svg");
  await expect(picker.getByRole("tab", { name: "Claude Desktop", exact: true }).locator("img")).toHaveAttribute("src", "/icons/claude-desktop.svg");
  expect(await picker.getByRole("tab").evaluateAll((elements) => elements.every((element) => {
    const style = getComputedStyle(element);
    const icon = element.querySelector("img");
    return style.borderTopWidth === "0px" && style.borderRightWidth === "0px"
      && style.borderBottomWidth === "0px" && style.borderLeftWidth === "0px"
      && icon?.complete && icon.naturalWidth > 0;
  }))).toBe(true);
  await chatgpt.focus();
  await chatgpt.press("ArrowDown");
  const claude = picker.getByRole("tab", { name: "Claude Desktop", exact: true });
  await expect(claude).toBeFocused();
  await expect(page.locator('.application-launch-path[data-application="claude-desktop"]')).toBeVisible();
  await claude.press("ArrowDown");
  await expect(page.getByRole("tab", { name: "OpenCode", exact: true })).toBeFocused();
  await expect(page.locator(".gateway-opencode-development")).toContainText("In development");
  await apiTab.click();
  await page.getByRole("button", { name: "Stop API", exact: true }).click();
  await expect(page.getByRole("button", { name: "Start API", exact: true })).toBeVisible();
});

test("application navigation stays readable with the sidebar expanded in a narrow window", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await installTauriMock(page, { locale: "en", mode: "local", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Access", exact: true }).click();
  await page.getByRole("tab", { name: "Applications", exact: true }).click();
  await page.getByRole("button", { name: "Expand sidebar", exact: true }).click();
  const row = page.locator(".application-workspace .integration-picker .relay-tabs");
  await expect(row).toHaveAttribute("aria-orientation", "vertical");
  expect(await row.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
  expect(await row.getByRole("tab").evaluateAll((elements) => elements.every((element) =>
    element.getBoundingClientRect().height >= 42 && element.scrollWidth <= element.clientWidth))).toBe(true);
  const chatgpt = row.getByRole("tab", { name: "ChatGPT", exact: true });
  await chatgpt.focus();
  await chatgpt.press("End");
  const opencode = row.getByRole("tab", { name: "OpenCode", exact: true });
  await expect(opencode).toBeFocused();
  expect(await opencode.evaluate((element) => {
    const box = element.getBoundingClientRect();
    const row = element.parentElement!.getBoundingClientRect();
    return box.left >= row.left - 1 && box.right <= row.right + 1;
  })).toBe(true);
});

for (const client of [
  { name: "ChatGPT", kind: "codex" },
  { name: "Excel / Basis Points", kind: "excel_bps" },
] as const) {
  test(`Connections signs in using ${client.name} without an extra completion dialog`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", mode: "local", populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.getByRole("button", { name: "Sign in", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Sign in", exact: true });
    await dialog.getByText(client.name, { exact: true }).click();
    await expect(dialog.getByRole("radio", { name: client.name, exact: true })).toBeChecked();
    await expect.poll(() => page.evaluate(() => (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }>;
    }).__TAURI_TEST_INVOKES__.findLast((call) => call.command === "start_codex_oauth")?.args))
      .toEqual({ openBrowser: false, ...(client.kind === "excel_bps" ? { clientKind: client.kind } : {}) });
    await emitTauriEvent(page, "relay-oauth-status", { loginId: "oauth_synthetic", status: "callback_received" });
    await expect(dialog).not.toBeVisible();
    await expect(page.getByRole("dialog", { name: "Account added", exact: true })).toHaveCount(0);
    await expect(page.getByRole("heading", { name: "Connections", exact: true })).toBeVisible();
    await expect(page.getByRole("tab", { name: "Accounts", exact: true })).toHaveAttribute("aria-selected", "true");
  });
}

test("Accounts keeps behavior settings beside its compact provider navigation", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "remote", populated: true, remoteFeatures: ["block_degraded_routes"] });
  await page.goto("/");
  await page.getByRole("button", { name: "Access", exact: true }).click();
  await expect(page.getByRole("tab", { name: "Server API", exact: true })).toBeVisible();
  await page.getByRole("tab", { name: "Accounts", exact: true }).click();
  const accounts = page.getByRole("tabpanel", { name: "Accounts", exact: true });
  await expect(accounts.getByRole("checkbox", { name: "Reject model substitution", exact: true })).toBeChecked();
  await expect(accounts.getByRole("tablist")).toHaveAttribute("aria-orientation", "vertical");
  await expect(accounts.getByRole("tab")).toHaveText(["ChatGPT"]);
  await expect(accounts.getByRole("button", { name: "Sign in", exact: true })).toHaveCount(0);
  await expect(accounts.getByRole("button", { name: "My connections", exact: true })).toHaveCount(0);
  await expect(accounts.getByRole("article")).toHaveCount(0);
  expect(await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string }>;
  }).__TAURI_TEST_INVOKES__.some((call) => call.command === "start_codex_oauth"))).toBe(false);
});

test("saved feedback and update appear above Recovery in both sidebar states", async ({ page }) => {
  await page.setViewportSize({ width: 840, height: 560 });
  await installTauriMock(page, { locale: "ru", theme: "dark", mode: "local", populated: true, updateVersion: "1.1.3" });
  await page.goto("/");
  await page.getByRole("button", { name: "Доступ", exact: true }).click();
  await page.getByRole("tab", { name: "Аккаунты", exact: true }).click();
  await page.getByRole("checkbox", { name: "Отклонять подмену модели", exact: true }).click();
  const feedback = page.getByRole("status", { name: "Сохранено.", exact: true });
  await expect(feedback).toBeVisible();
  await expect(page.getByRole("button", { name: "Открыть обновление 1.1.3", exact: true })).toBeVisible();
  for (const state of ["collapsed", "expanded"]) {
    if (state === "expanded") await page.getByRole("button", { name: "Развернуть боковую панель", exact: true }).click();
    for (const button of await page.getByRole("navigation").getByRole("button").all()) {
      await expect(button).toBeInViewport({ ratio: 1 });
    }
    expect(await page.locator(".relay-sidebar").evaluate((element) => {
      const selectors = [".sidebar-feedback", ".sidebar-update-row", ".sidebar-nav-utilities", ".sidebar-footer"];
      const boxes = selectors.map((selector) => element.querySelector(selector)!.getBoundingClientRect());
      return boxes.every((box, index) => box.top >= 36 && box.bottom <= innerHeight
        && (index === 0 || boxes[index - 1]!.bottom <= box.top));
    })).toBe(true);
    await page.screenshot({ path: `output/playwright/access-sidebar-feedback-${state}-ru-dark.png` });
  }
  await page.getByRole("navigation", { name: "Обслуживание", exact: true }).getByRole("button", { name: "Восстановление", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Восстановление", exact: true })).toBeVisible();
});

for (const locale of ["ru", "en"] as const) {
  for (const theme of ["dark", "light"] as const) {
    for (const viewport of [{ width: 1280, height: 900 }, { width: 840, height: 560 }, { width: 390, height: 844 }]) {
      test(`Access layout ${locale} ${theme} ${viewport.width}`, async ({ page }) => {
        const russian = locale === "ru";
        await page.setViewportSize(viewport);
        await installTauriMock(page, { locale, theme, mode: "local", populated: true, accountCount: 4 });
        await page.goto("/");
        await page.getByRole("button", { name: russian ? "Доступ" : "Access", exact: true }).click();
        for (const [id, label] of [
          ["api", russian ? "Локальный API" : "Local API"],
          ["applications", russian ? "Приложения" : "Applications"],
          ["accounts", russian ? "Аккаунты" : "Accounts"],
        ]) {
          await page.getByRole("tab", { name: label, exact: true }).click();
          await expect(page.locator(".relay-content")).toBeVisible();
          expect(await page.locator(".relay-content").evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
          const surfaces = page.locator(".integration-picker, .integration-account-policy, .gateway-api-field, .gateway-setting-row, .oauth-binding-settings");
          expect(await surfaces.evaluateAll((elements) => elements.every((element) => {
            const box = element.getBoundingClientRect();
            return box.left >= 0 && box.right <= innerWidth + 1 && element.scrollWidth <= element.clientWidth + 1;
          }))).toBe(true);
          await page.screenshot({ path: `output/playwright/access-${id}-${locale}-${theme}-${viewport.width}.png` });
          if (id === "applications") {
            const applications = page.locator(".application-workspace .integration-picker");
            await expect(applications.getByRole("tab")).toHaveText(["ChatGPT", "Claude Desktop", "OpenCode"]);
            expect(await applications.getByRole("tab").evaluateAll((elements) => elements.every((element, index) => {
              const box = element.getBoundingClientRect();
              const previous = elements[index - 1]?.getBoundingClientRect();
              const icon = element.querySelector("img")!.getBoundingClientRect();
              const range = document.createRange();
              range.selectNodeContents(element.lastChild!);
              return (!previous || previous.bottom <= box.top && previous.left === box.left)
                && box.height >= 42 && box.height < 64
                && [...range.getClientRects()].every((label) => label.left > icon.right && label.right <= box.right - 11)
                && Math.abs(icon.top + icon.height / 2 - box.top - box.height / 2) < 1;
            }))).toBe(true);
            expect(await page.locator(".application-workspace").evaluate((element) => {
              const picker = element.querySelector(".integration-picker")!.getBoundingClientRect();
              const settings = element.querySelector(".integrations-settings")!.getBoundingClientRect();
              return getComputedStyle(element).gridTemplateColumns.split(" ").length === 1
                ? settings.top >= picker.bottom : settings.left >= picker.right && Math.abs(settings.top - picker.top) < 1;
            })).toBe(true);
            await applications.getByRole("tab", { name: "Claude Desktop", exact: true }).click();
            await expect(page.locator('.application-launch-path[data-application="claude-desktop"]')).toBeVisible();
            await page.screenshot({ path: `output/playwright/access-claude-desktop-${locale}-${theme}-${viewport.width}.png` });
          }
        }
        const expand = page.getByRole("button", { name: russian ? "Развернуть боковую панель" : "Expand sidebar", exact: true });
        if (await expand.isVisible()) await expand.click();
        const settings = page.getByRole("navigation", { name: russian ? "Обслуживание" : "Maintenance", exact: true })
          .getByRole("button", { name: russian ? "Настройки" : "Settings", exact: true });
        await settings.scrollIntoViewIfNeeded();
        await expect(settings).toBeInViewport();
      });
    }
  }
}
