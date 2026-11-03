import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const locale of ["en", "ru"] as const) {
  for (const theme of ["light", "dark"] as const) {
    test(`${locale} ${theme} overview aligns token scales and mirrors numeric axes`, async ({ page }) => {
      await installTauriMock(page, { mode: "local", locale, theme, populated: true });
      await page.goto("/");
      await expect(page.locator(".overview-analytics")).toHaveClass(/has-data/);
      await expect(page.locator(".overview-analytics")).toHaveAttribute("aria-busy", "false");

      const tones = ["cost", "requests", "speed", "e2e-speed", "weekly-capacity"];
      for (const viewport of [{ width: 1160, height: 900 }, { width: 390, height: 844 }]) {
        await page.setViewportSize(viewport);
        const charts = page.locator(".overview-chart-stack > .overview-chart:not(.tokens)");
        await expect(charts).toHaveCount(tones.length);
        for (const tone of tones) {
          const chart = page.locator(`.overview-chart.${tone}`);
          const left = chart.locator(".overview-chart-y-axis:not(.overview-chart-y-axis-right)");
          const right = chart.locator(".overview-chart-y-axis-right");
          await expect(right).toHaveAttribute("aria-hidden", "true");
          await expect(right.locator("span")).toHaveText(await left.locator("span").allTextContents());
          await expect(right.locator("span")).toHaveCount(3);
          await expect.poll(() => chart.locator(".overview-chart-body").evaluate((node) => node.scrollWidth <= node.clientWidth)).toBe(true);
          const geometry = await chart.locator(".overview-chart-body").evaluate((node) => {
            const left = node.querySelector(".overview-chart-y-axis:not(.overview-chart-y-axis-right)")!;
            const right = node.querySelector(".overview-chart-y-axis-right")!;
            const plot = node.querySelector(".overview-chart-plot")!.getBoundingClientRect();
            return {
              leftEdge: left.getBoundingClientRect().right,
              rightEdge: right.getBoundingClientRect().left,
              plotLeft: plot.left,
              plotRight: plot.right,
              leftTicks: Array.from(left.children, (tick) => tick.getBoundingClientRect().top),
              rightTicks: Array.from(right.children, (tick) => tick.getBoundingClientRect().top),
            };
          });
          expect(geometry.leftEdge).toBeLessThanOrEqual(geometry.plotLeft);
          expect(geometry.rightEdge).toBeGreaterThanOrEqual(geometry.plotRight);
          for (let index = 0; index < 3; index += 1) {
            expect(Math.abs(geometry.leftTicks[index] - geometry.rightTicks[index])).toBeLessThan(1);
          }
          await chart.screenshot({ path: `output/playwright/overview-axis-${tone}-${locale}-${theme}-${viewport.width}.png` });
        }

        const tokens = page.locator(".overview-token-trend");
        await expect(tokens.locator(".overview-chart-y-axis-right")).toHaveCount(0);
        await expect(tokens.locator(".overview-token-trend-axis span")).toHaveCount(3);
        await expect(tokens.locator(".overview-token-trend-rate-axis span")).toHaveText(["100%", "50%", "0%"]);
        await expect.poll(() => tokens.locator(".overview-token-trend-body").evaluate((node) => node.scrollWidth <= node.clientWidth)).toBe(true);
        await expect.poll(() => page.locator(".overview-chart-stack").evaluate((node) => {
          const tokenPlot = node.querySelector(".overview-token-trend-plot")!.getBoundingClientRect();
          const referencePlot = node.querySelector(".overview-chart.cost .overview-chart-plot")!.getBoundingClientRect();
          const leftTicks = node.querySelectorAll(".overview-token-trend-axis > span");
          const rightTicks = node.querySelectorAll(".overview-token-trend-rate-axis > span");
          const referenceLeft = node.querySelector(".overview-chart.cost .overview-chart-y-axis > span")!.getBoundingClientRect();
          const referenceRight = node.querySelector(".overview-chart.cost .overview-chart-y-axis-right > span")!.getBoundingClientRect();
          const leftGap = referencePlot.left - referenceLeft.right;
          const rightGap = referenceRight.left - referencePlot.right;
          return Math.max(
            ...Array.from(leftTicks, (tick) => Math.abs(tokenPlot.left - tick.getBoundingClientRect().right - leftGap)),
            ...Array.from(rightTicks, (tick) => Math.abs(tick.getBoundingClientRect().left - tokenPlot.right - rightGap)),
          );
        })).toBeLessThan(1);
        await tokens.screenshot({ path: `output/playwright/overview-axis-tokens-${locale}-${theme}-${viewport.width}.png` });
      }
    });
  }
}
