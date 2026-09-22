import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const failedStep of ["state", "membership"] as const) {
  test(`source creation retry after failed ${failedStep} reuses the saved source`, async ({ page }) => {
    await installTauriMock(page, { populated: false, sourceFollowupError: failedStep });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.getByRole("button", { name: "Add member", exact: true }).first().click();
    await page.getByRole("dialog").getByRole("button", { name: "Add API source", exact: true }).click();
    const form = page.getByRole("dialog", { name: "Add API source", exact: true });
    await form.getByRole("radio", { name: "OpenAI", exact: true }).click();
    await form.getByLabel("Upstream API key").fill("synthetic-retry-key");
    await form.getByRole("button", { name: "Save", exact: true }).click();
    const error = page.getByRole("dialog", { name: "Error details" });
    await expect(error).toBeVisible();
    await error.locator("footer").getByRole("button", { name: "Close", exact: true }).click();
    await page.getByRole("dialog", { name: failedStep === "state" ? "Add API source" : "Edit source", exact: true }).getByRole("button", { name: "Save", exact: true }).click();
    await expect(error).toBeHidden();
    const editor = page.getByRole("dialog", { name: "Edit source", exact: true });
    if (failedStep === "state") {
      await expect(editor).toBeVisible();
      await editor.getByRole("button", { name: "Cancel", exact: true }).click();
    } else {
      await expect(editor).toBeHidden();
    }
    await expect(page.locator(".pool-member-card")).toHaveCount(1);
    const calls = await page.evaluate(() => (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { sourceIds?: string[] } } }>;
    }).__TAURI_TEST_INVOKES__);
    expect(calls.filter((call) => call.command === "create_local_source")).toHaveLength(1);
    const memberships = calls.filter((call) => call.command === "set_local_pool_membership");
    expect(memberships).toHaveLength(failedStep === "membership" ? 2 : 1);
    expect(memberships.every((call) => call.args.input?.sourceIds?.join() === "source_created_1")).toBe(true);
  });
}

test("provider picker supports keyboard selection and preserves edits to the selected provider", async ({ page }) => {
  await installTauriMock(page, { mode: "zenith", locale: "en", populated: false, readyConnected: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Add source", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Add source" });
  const first = dialog.getByRole("radio", { name: "OpenAI", exact: true });
  await first.focus();
  await first.press("ArrowRight");
  const router = dialog.getByRole("radio", { name: "OpenRouter", exact: true });
  await expect(router).toBeFocused();
  await expect(router).toBeChecked();
  await expect(dialog.getByLabel("API address", { exact: true })).toHaveValue("https://openrouter.ai/api/v1");
  await router.press("End");
  const custom = dialog.getByRole("radio", { name: "Custom API", exact: true });
  await expect(custom).toBeFocused();
  await expect(custom).toBeChecked();

  await dialog.getByLabel("Upstream API key").fill("synthetic-provider-key");
  await dialog.getByLabel("API address", { exact: true }).fill("https://api.example.invalid/v1");
  await expect(dialog.getByRole("button", { name: "Save", exact: true })).toBeDisabled();
  await dialog.getByLabel("Name", { exact: true }).fill("Work API");
  await custom.click();
  await expect(dialog.getByLabel("Name", { exact: true })).toHaveValue("Work API");
  await expect(dialog.getByLabel("API address", { exact: true })).toHaveValue("https://api.example.invalid/v1");
  await expect(dialog.getByLabel("Upstream API key")).toHaveValue("synthetic-provider-key");
  await expect(dialog.getByRole("radio")).toHaveCount(4);
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Edit source" })).toBeVisible();
  const input = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: unknown } }>;
  }).__TAURI_TEST_INVOKES__.find((call) => call.command === "create_local_source")?.args.input);
  expect(input).toMatchObject({ name: "Work API", baseUrl: "https://api.example.invalid/v1", apiKey: "synthetic-provider-key", protocolBindings: [] });
});

for (const theme of ["light", "dark"] as const) {
  for (const viewport of [{ width: 1160, height: 760 }, { width: 840, height: 560 }, { width: 390, height: 844 }]) {
    test(`provider setup stays compact and usable ${theme} ${viewport.width}`, async ({ page }) => {
      await installTauriMock(page, { mode: "zenith", locale: "ru", theme, populated: false, readyConnected: false });
      await page.setViewportSize(viewport);
      await page.goto("/");
      await page.getByRole("button", { name: "Подключения", exact: true }).click();
      await page.getByRole("button", { name: "Добавить источник", exact: true }).click();
      await expect(page.locator("#splash-screen")).toHaveCount(0);
      const dialog = page.getByRole("dialog", { name: "Добавить источник" });
      await page.screenshot({ path: `output/playwright/provider-picker-${theme}-${viewport.width}.png` });
      await dialog.getByRole("radio", { name: "Свой API", exact: true }).click();
      await dialog.getByLabel("Ключ внешнего API").fill("synthetic-preview-key");
      await dialog.getByLabel("Адрес API", { exact: true }).fill("https://api.example.invalid/v1");
      await dialog.getByLabel("Название", { exact: true }).fill("Рабочий API");
      await expect(dialog.getByRole("button", { name: "Сохранить", exact: true })).toBeEnabled();
      expect(await dialog.evaluate((element) => {
        const box = element.getBoundingClientRect();
        const body = element.querySelector(".relay-dialog-body")!;
        return box.width <= 620 && box.left >= 0 && box.right <= innerWidth && box.top >= 36 && box.bottom <= innerHeight && body.scrollWidth <= body.clientWidth;
      })).toBe(true);
      expect(await dialog.locator("input, [role=radio], footer").evaluateAll((items) => items.every((item) => {
        const box = item.getBoundingClientRect();
        return box.left >= 0 && box.right <= innerWidth && box.top >= 36 && box.bottom <= innerHeight && item.scrollWidth <= item.clientWidth;
      }))).toBe(true);
      await page.screenshot({ path: `output/playwright/provider-custom-${theme}-${viewport.width}.png`, animations: "disabled" });
      await dialog.getByRole("button", { name: "Сохранить", exact: true }).click();
      await expect(page.getByRole("dialog", { name: "Изменить источник" })).toBeVisible();
    });
  }
}
