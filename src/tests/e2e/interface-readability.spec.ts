import { expect, test, type Locator } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

async function textContrast(locator: Locator) {
  return locator.evaluateAll((elements) => elements.map((element) => {
    const luminance = (color: Uint8ClampedArray) => Array.from(color).slice(0, 3)
      .map((value) => value / 255)
      .map((value) => value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4)
      .reduce((sum, value, index) => sum + value * [0.2126, 0.7152, 0.0722][index], 0);
    // Let the browser resolve color-mix()/color(srgb ...) and alpha layers.
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = 1;
    const context = canvas.getContext("2d", { willReadFrequently: true })!;
    const layers: string[] = [];
    let ancestor: Element | null = element;
    while (ancestor) {
      layers.push(getComputedStyle(ancestor).backgroundColor);
      ancestor = ancestor.parentElement;
    }
    for (const layer of layers.reverse()) {
      context.fillStyle = layer;
      context.fillRect(0, 0, 1, 1);
    }
    const background = context.getImageData(0, 0, 1, 1).data;
    if (background[3] !== 255) throw new Error("No opaque background behind text");
    const style = getComputedStyle(element);
    context.fillStyle = style.color;
    context.fillRect(0, 0, 1, 1);
    const foreground = luminance(context.getImageData(0, 0, 1, 1).data);
    const behind = luminance(background);
    return {
      text: element.textContent?.trim(),
      size: parseFloat(style.fontSize),
      contrast: (Math.max(foreground, behind) + 0.05) / (Math.min(foreground, behind) + 0.05),
    };
  }));
}

const scenarios = (["system", "fallback"] as const).flatMap((font) =>
  (["en", "ru"] as const).map((locale) => ({ font, locale })));

for (const { font, locale } of scenarios) {
  for (const theme of ["light", "dark"] as const) {
    test(`chart labels and account captions stay readable: ${locale} ${theme} ${font}`, async ({ page }, testInfo) => {
      await installTauriMock(page, {
        mode: "local", locale, theme, populated: true, accountCount: 4,
        quotaAvailable: true, supplementalQuota: true, subscriptionExpiresInMs: 28 * 86_400_000,
      });
      await page.goto("/");
      if (font === "fallback") {
        await page.addStyleTag({ content: ".app { font-family: Verdana, sans-serif !important; }" });
      }
      await expect(page.locator(".overview-analytics")).toHaveClass(/has-data/);
      await expect(page.locator(".overview-analytics")).toHaveAttribute("aria-busy", "false");
      const labels = page.locator([
        ".overview-token-trend-legend > span", ".overview-token-trend-summary small",
        ".overview-token-trend-axis", ".overview-token-trend-rate-axis",
        ".overview-chart-y-axis", ".overview-chart-x-axis",
      ].join(", "));
      expect(await labels.count()).toBeGreaterThan(10);
      for (const width of [1160, 840]) {
        await page.setViewportSize({ width, height: 760 });
        for (const label of await textContrast(labels)) {
          expect(label.size, label.text).toBeGreaterThanOrEqual(12);
          expect(label.contrast, label.text).toBeGreaterThanOrEqual(4.5);
        }
        const summaries = page.locator(".overview-chart-summary");
        for (const summary of await textContrast(summaries)) {
          expect(summary.contrast, summary.text).toBeGreaterThanOrEqual(3);
        }
        await expect.poll(() => page.locator(".relay-content").evaluate((element) => element.scrollWidth - element.clientWidth)).toBeLessThanOrEqual(1);
      }
      await page.screenshot({ path: testInfo.outputPath("overview.png"), animations: "disabled" });

      await page.getByRole("button", { name: locale === "en" ? "Connections" : "Подключения", exact: true }).click();
      const card = page.locator(".account-card").first();
      await expect(card.locator(".account-value-strip")).toBeVisible();
      const captions = card.locator(".quota-meter-heading small, .account-value-strip dt, .account-subscription-line");
      expect(await captions.count()).toBeGreaterThan(5);
      for (const width of [1160, 840]) {
        await page.setViewportSize({ width, height: 760 });
        for (const caption of await textContrast(captions)) {
          expect(caption.size, caption.text).toBeGreaterThanOrEqual(11);
          expect(caption.contrast, caption.text).toBeGreaterThanOrEqual(4.5);
        }
        const clipped = await card.locator(".quota-meter-heading small, .account-value-strip dt").evaluateAll((elements) => elements
          .filter((element) => element.scrollWidth > element.clientWidth + 1)
          .map((element) => element.textContent?.trim()));
        expect(clipped).toEqual([]);
        await expect.poll(() => card.evaluate((element) => element.scrollWidth - element.clientWidth)).toBeLessThanOrEqual(1);
      }
      await page.screenshot({ path: testInfo.outputPath("connections.png"), animations: "disabled" });
    });
  }
}
