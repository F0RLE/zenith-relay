import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

test("catalog families from any provider keep the backend block order", async ({ page }) => {
  const ids = ["future-new", "future-old", "other-family", "gpt-5.4", "gpt-5.4-mini"];
  await installTauriMock(page, {
    mode: "local", locale: "en", populated: true, serverModelOrder: ids,
    modelMetadata: {
      "future-new": { catalogProvider: "future-company", catalogFamily: "future-line", catalogName: "Future 2" },
      "future-old": { catalogProvider: "future-company", catalogFamily: "future-line", catalogName: "Future 1" },
      "other-family": { catalogProvider: "future-company", catalogFamily: "another-line", catalogName: "Another 3" },
    },
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Model Rules", exact: true }).click();
  const rows = page.locator(".model-rules tr[data-model-id]");
  await expect.poll(() => rows.evaluateAll((items) => items.map((item) => item.getAttribute("data-model-id")))).toEqual(ids);
  await expect(page.locator(".model-group-content strong")).toHaveText(["Future Company", "OpenAI"]);
});

for (const mode of ["local", "remote"] as const) {
  test(`${mode} model order resets groups and rows without changing model policy`, async ({ page }) => {
    await installTauriMock(page, {
      mode, locale: "en", populated: true,
      serverModelOrder: ["gpt-5.4", "gpt-5.4-mini", "claude-opus-4-8", "gemini-3.1-pro-preview"],
      modelOrderDelayMs: 300,
    });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.getByRole("tab", { name: "Model Rules", exact: true }).click();
    const rows = page.locator(".model-rules tr[data-model-id]");
    const ids = () => rows.evaluateAll((items) => items.map((item) => item.getAttribute("data-model-id")));
    const original = await ids();
    const reset = page.getByRole("button", { name: "Reset model order", exact: true });
    const groups = page.locator(".model-group-content strong");

    await rows.nth(1).dragTo(rows.first());
    await expect(reset).toBeDisabled();
    await expect(reset).toBeEnabled();
    await expect.poll(ids).toEqual(["gpt-5.4-mini", "gpt-5.4", "claude-opus-4-8", "gemini-3.1-pro-preview"]);
    await page.locator(".model-group-row").first().dragTo(page.locator(".model-group-row").last());
    await expect(reset).toBeEnabled();
    await expect(groups).toHaveText(["Anthropic", "Google", "OpenAI"]);
    const claude = page.locator('[data-model-id="claude-opus-4-8"]');
    await claude.locator("[data-model-toggle]").click();
    await expect(claude).toHaveAttribute("data-enabled", "false");

    await reset.click();
    await expect(reset).toBeDisabled();
    await expect(reset).toBeEnabled();
    await expect.poll(ids).toEqual(original);
    await expect(groups).toHaveText(["OpenAI", "Anthropic", "Google"]);
    await expect(claude).toHaveAttribute("data-enabled", "false");
    await page.getByRole("tab", { name: "Members", exact: true }).click();
    await page.getByRole("tab", { name: "Model Rules", exact: true }).click();
    await expect.poll(ids).toEqual(original);

    await page.screenshot({ path: `output/playwright/model-order-${mode}-desktop.png` });
    await page.setViewportSize({ width: 390, height: 844 });
    await page.getByRole("button", { name: "Collapse sidebar", exact: true }).click();
    await expect(reset).toBeVisible();
    const bounds = await reset.boundingBox();
    expect(bounds && bounds.x >= 0 && bounds.x + bounds.width <= 390).toBeTruthy();
    await page.screenshot({ path: `output/playwright/model-order-${mode}-mobile.png` });
  });

  test(`${mode} failed model reorder restores saved positions`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, modelOrderError: true, modelOrderDelayMs: 400 });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.getByRole("tab", { name: "Model Rules", exact: true }).click();
    const rows = page.locator(".model-rules tr[data-model-id]");
    const ids = () => rows.evaluateAll((items) => items.map((item) => item.getAttribute("data-model-id")));
    const original = await ids();
    const reset = page.getByRole("button", { name: "Reset model order", exact: true });
    await rows.first().dragTo(rows.last());
    await expect(reset).toBeDisabled();
    await expect.poll(ids).toEqual([...original].reverse());
    await expect(reset).toBeEnabled();
    await expect.poll(ids).toEqual(original);
    await reset.click();
    await expect(reset).toBeDisabled();
    await expect(reset).toBeEnabled();
    await expect.poll(ids).toEqual(original);
  });
}

test("old remote server does not offer an unsupported order reset", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true, remoteFeatures: ["models", "sources"] });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Model Rules", exact: true }).click();
  await expect(page.getByRole("button", { name: "Reset model order", exact: true })).toHaveCount(0);
});

test("model order reset fits the Russian dark compact toolbar", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", theme: "dark", populated: true, mixedModels: true });
  await page.setViewportSize({ width: 840, height: 560 });
  await page.goto("/");
  await page.getByRole("button", { name: "Пул", exact: true }).click();
  await page.getByRole("tab", { name: "Правила моделей", exact: true }).click();
  const reset = page.getByRole("button", { name: "Сбросить порядок моделей", exact: true });
  await expect(reset).toBeVisible();
  await reset.hover();
  await expect(page.getByRole("tooltip")).toContainText("Сбросить порядок моделей");
  await page.screenshot({ path: "output/playwright/model-order-ru-dark.png" });
});
