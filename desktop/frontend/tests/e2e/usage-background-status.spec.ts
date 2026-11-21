import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const scenario of [
  { locale: "ru", theme: "dark", width: 1660 },
  { locale: "en", theme: "light", width: 390 },
] as const) {
  test(`background requests keep the candidate and color their status robot: ${scenario.locale} ${scenario.width}`, async ({ page }) => {
    await page.setViewportSize({ width: scenario.width, height: 900 });
    await installTauriMock(page, {
      ...scenario, mode: "local", populated: true, accountLabel: "Synthetic candidate",
      usageFilterHistory: [
        { wireApi: "responses", transport: "http", errorCategory: "codex_task_title", success: true },
        { wireApi: "responses", transport: "http", errorCategory: "codex_memory_extraction", success: false },
        { wireApi: "responses", transport: "http", errorCategory: "codex_background_blocked_activity_summary", success: false },
        { wireApi: "responses", transport: "http", errorCategory: null, success: true },
        { wireApi: "responses", transport: "http", errorCategory: "upstream_rate_limited", success: false },
      ],
    });
    await page.goto("/");
    await page.getByRole("button", { name: scenario.locale === "ru" ? "Использование" : "Usage", exact: true }).click();
    const table = page.locator(".usage-request-table");
    const rows = table.locator("tbody tr");
    await expect(rows).toHaveCount(5);
    const background = rows.filter({ has: page.locator('[data-column="status"] .lucide-bot') });
    await expect(background).toHaveCount(3);
    await expect(table.locator('[data-column="model"] svg')).toHaveCount(0);
    await expect(table.locator('tbody [data-column="connection"]')).toHaveText(Array(5).fill("Synthetic candidate"));
    for (const [status, token] of [["ready", "--relay-success"], ["error", "--relay-danger"], ["warning", "--relay-warning"]] as const) {
      const icon = table.locator(`[data-column="status"] .relay-status-icon[data-status="${status}"]`).filter({ has: page.locator(".lucide-bot") });
      await expect(icon).toHaveCount(1);
      expect(await icon.evaluate((element, cssToken) => {
        const probe = document.createElement("span");
        probe.style.color = `var(${cssToken})`;
        element.append(probe);
        const expected = getComputedStyle(probe).color;
        probe.remove();
        return getComputedStyle(element.querySelector("svg")!).color === expected;
      }, token)).toBe(true);
    }
    await expect(table.locator('[data-column="status"] .lucide-circle-check')).toHaveCount(1);
    await expect(table.locator('[data-column="status"] .lucide-circle-alert')).toHaveCount(1);
    await page.screenshot({ path: test.info().outputPath(`background-status-${scenario.locale}-${scenario.width}.png`), fullPage: true });
    const successful = background.filter({ has: page.locator('.relay-status-icon[data-status="ready"]') });
    await successful.locator('[data-column="request"] button').click();
    const details = page.getByRole("dialog");
    await expect(details.locator('.request-details-status[data-status="success"] .lucide-bot')).toBeVisible();
    await expect(details).toContainText("Synthetic candidate");
  });
}
