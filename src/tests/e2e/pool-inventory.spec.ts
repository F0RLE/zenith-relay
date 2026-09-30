import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const mode of ["local", "remote"] as const) {
  test(`${mode} unavailable pool inventory retains names, groups and editable reasoning`, async ({ page }) => {
    await installTauriMock(page, {
      mode, locale: "en", populated: true, sourceEnabled: false,
      gatewayRunning: false, accountHealth: "disabled",
      serverModelOrder: ["gpt-5.4", "claude-opus-4-8", "gemini-3.1-pro-preview"],
      accountModels: ["gpt-5.4"],
      modelReasoning: { "claude-opus-4-8": ["low", "medium", "high"] },
      modelProtocolRoutes: { "claude-opus-4-8": [] },
    });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.getByRole("tab", { name: "Model Rules", exact: true }).click();
    const rows = page.locator(".model-rules tr[data-model-id]");
    await expect(rows).toHaveCount(3);
    await expect(page.locator(".model-group-content strong")).toHaveText(["OpenAI", "Anthropic", "Google"]);
    const claude = page.locator('[data-model-id="claude-opus-4-8"]');
    await expect(claude.locator(".model-rule-identity strong")).toHaveText("Claude Opus 4.8");
    await claude.locator("[data-model-reasoning-edit]").click();
    const dialog = page.getByRole("dialog", { name: "Reasoning modes" });
    await expect(dialog.getByRole("checkbox")).toHaveText(["Low", "Medium", "High"]);
    await dialog.getByRole("checkbox", { name: "High", exact: true }).click();
    await expect(dialog.getByRole("checkbox", { name: "High", exact: true })).toHaveAttribute("aria-checked", "false");
    await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
    await claude.locator("[data-model-toggle]").click();
    await expect(claude).toHaveAttribute("data-enabled", "false");
    await expect(claude.locator(".model-rule-identity strong")).toHaveText("Claude Opus 4.8");
    await page.getByRole("tab", { name: "Members", exact: true }).click();
    await page.getByRole("tab", { name: "Model Rules", exact: true }).click();
    await expect(rows).toHaveCount(3);
    await expect(claude).toHaveAttribute("data-enabled", "false");
    await claude.locator("[data-model-reasoning-edit]").click();
    await expect(dialog.getByRole("checkbox", { name: "High", exact: true })).toHaveAttribute("aria-checked", "false");
    await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
    await page.screenshot({ path: `output/playwright/pool-inventory-${mode}.png` });
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(claude).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  });
}
