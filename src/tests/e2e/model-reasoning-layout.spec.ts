import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

const levels = ["none", "low", "medium", "high", "xhigh", "max"];

async function openReasoning(page: Page, locale: "ru" | "en") {
  await page.getByRole("button", { name: locale === "ru" ? "Пул" : "Pool", exact: true }).click();
  await page.getByRole("tab", { name: locale === "ru" ? "Правила моделей" : "Model Rules", exact: true }).click();
  await page.locator('[data-model-id="gpt-5.4"] [data-model-reasoning-edit]').click();
  return page.getByRole("dialog");
}

async function savedLevels(page: Page) {
  return page.evaluate(() => {
    const calls = (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: {
        allowedLevels?: string[]; action?: { type?: string }; payload?: { allowedLevels?: string[] };
      } } }>;
    }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "set_local_model_reasoning")?.args.input?.allowedLevels;
  });
}

for (const width of [1160, 520, 390, 320]) {
  for (const theme of ["light", "dark"] as const) {
    for (const locale of ["ru", "en"] as const) {
      test(`reasoning tiles stay aligned without checkmarks in ${locale} ${theme} at ${width}px`, async ({ page }, testInfo) => {
        await page.setViewportSize({ width, height: 844 });
        await installTauriMock(page, { locale, theme, mode: "local", populated: true, modelReasoning: { "gpt-5.4": levels } });
        await page.goto("/");
        const dialog = await openReasoning(page, locale);
        const options = dialog.getByRole("checkbox");
        await expect(options).toHaveCount(levels.length);
        await expect(dialog.locator(".model-reasoning-options svg")).toHaveCount(0);
        for (const option of await options.all()) await expect(option).toBeChecked();

        const layout = await dialog.evaluate((element) => {
          const body = element.querySelector(".relay-dialog-body")!;
          const grid = element.querySelector(".model-reasoning-options")!;
          const buttons = [...grid.querySelectorAll("button")];
          const bounds = grid.getBoundingClientRect();
          const first = buttons[0]!.getBoundingClientRect();
          return {
            columns: getComputedStyle(grid).gridTemplateColumns.split(" ").length,
            aligned: buttons.every((button) => {
              const rect = button.getBoundingClientRect();
              const label = button.querySelector("span")!.getBoundingClientRect();
              return Math.abs(rect.width - first.width) < 1 && Math.abs(rect.height - first.height) < 1
                && rect.left >= bounds.left - 1 && rect.right <= bounds.right + 1
                && label.left >= rect.left + 10 && label.right <= rect.right - 10
                && label.height <= Number.parseFloat(getComputedStyle(button).lineHeight) + 1;
            }),
            noOverflow: body.scrollWidth <= body.clientWidth + 1,
          };
        });
        expect(layout).toEqual({ columns: width <= 480 ? 2 : 3, aligned: true, noOverflow: true });

        await options.first().focus();
        await page.keyboard.press("Tab");
        await expect(options.nth(1)).toBeFocused();
        await expect(options.nth(1)).toHaveCSS("border-top-width", "2px");
        await page.keyboard.press("Space");
        await expect(options.nth(1)).not.toBeChecked();
        await options.last().click();
        await expect(options.last()).not.toBeChecked();
        await expect.poll(() => savedLevels(page)).toEqual(["none", "medium", "high", "xhigh"]);
        const selectedColor = await options.first().evaluate((button) => getComputedStyle(button).backgroundColor);
        await expect(options.nth(1)).not.toHaveCSS("background-color", selectedColor);
        await options.first().focus();
        await page.keyboard.press("Tab");
        await dialog.screenshot({ path: testInfo.outputPath("reasoning.png"), animations: "disabled" });

        await page.keyboard.press("Escape");
        await page.locator('[data-model-id="gpt-5.4"] [data-model-reasoning-edit]').click();
        await expect(options.nth(1)).not.toBeChecked();
        await expect(options.last()).not.toBeChecked();
        await expect(options.first()).toBeChecked();
      });
    }
  }
}
