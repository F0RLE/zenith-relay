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
      for (const viewport of [{ width: 1600, height: 1000 }, { width: 1160, height: 900 }, { width: 390, height: 844 }]) {
        await page.setViewportSize(viewport);
        const stack = page.locator(".overview-chart-stack");
        const layout = await stack.evaluate((node) => {
          const bounds = node.getBoundingClientRect();
          return {
            left: bounds.left,
            width: bounds.width,
            charts: Array.from(node.children, (chart) => {
              const rect = chart.getBoundingClientRect();
              return { left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom, width: rect.width };
            }),
          };
        });
        expect(layout.charts).toHaveLength(6);
        for (const chart of [layout.charts[0], layout.charts[5]]) {
          expect(Math.abs(chart.left - layout.left)).toBeLessThan(1);
          expect(Math.abs(chart.width - layout.width)).toBeLessThan(1);
        }
        if (viewport.width === 1600) {
          for (const [left, right] of [[layout.charts[1], layout.charts[2]], [layout.charts[3], layout.charts[4]]]) {
            expect(Math.abs(left.top - right.top)).toBeLessThan(1);
            expect(Math.abs(left.width - right.width)).toBeLessThan(1);
            expect(left.right).toBeLessThanOrEqual(right.left);
          }
          expect(layout.charts[0].bottom).toBeLessThanOrEqual(layout.charts[1].top);
          expect(layout.charts[2].bottom).toBeLessThanOrEqual(layout.charts[3].top);
          expect(layout.charts[4].bottom).toBeLessThanOrEqual(layout.charts[5].top);
        } else {
          for (const [index, chart] of layout.charts.entries()) {
            expect(Math.abs(chart.left - layout.left)).toBeLessThan(1);
            expect(Math.abs(chart.width - layout.width)).toBeLessThan(1);
            if (index > 0) expect(layout.charts[index - 1].bottom).toBeLessThanOrEqual(chart.top);
          }
        }
        await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
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
        await stack.screenshot({ path: `output/playwright/overview-layout-${locale}-${theme}-${viewport.width}.png` });
      }
    });
  }
}
