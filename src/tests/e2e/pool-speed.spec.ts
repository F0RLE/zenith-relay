import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

async function savedTiers(page: Page) {
  return page.evaluate(() => {
    const calls = (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { defaultServiceTier?: string; action?: { type?: string }; payload?: { defaultServiceTier?: string } } } }>;
    }).__TAURI_TEST_INVOKES__;
    return calls.flatMap(({ command, args }) => {
      if (command === "update_local_routing") return [args.input?.defaultServiceTier];
      if (command === "execute_remote_server_action" && args.input?.action?.type === "set_routing_policy") return [args.input.payload?.defaultServiceTier];
      return [];
    });
  });
}

function speedControl(page: Page) {
  return page.getByRole("radiogroup", { name: "Request speed" });
}

for (const mode of ["local", "remote"] as const) {
  test(`${mode} pool speed selects each mode and keeps it after leaving the page`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    const speed = speedControl(page);
    await expect(speed.getByRole("radio", { name: "Standard", exact: true })).toBeChecked();
    await speed.getByRole("radio", { name: "Standard", exact: true }).click();
    expect(await savedTiers(page)).toEqual([]);
    await speed.getByRole("radio", { name: "Fast", exact: true }).click();
    await expect(speed.getByRole("radio", { name: "Fast", exact: true })).toBeEnabled();
    await speed.getByRole("radio", { name: "Ultrafast", exact: true }).click();
    await expect(speed.getByRole("radio", { name: "Ultrafast", exact: true })).toBeEnabled();
    await expect.poll(() => savedTiers(page)).toEqual(["fast", "ultrafast"]);

    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await expect(speed.getByRole("radio", { name: "Ultrafast", exact: true })).toBeChecked();
    await speed.getByRole("radio", { name: "Ultrafast", exact: true }).press("ArrowLeft");
    await expect(speed.getByRole("radio", { name: "Fast", exact: true })).toBeEnabled();
    await speed.getByRole("radio", { name: "Fast", exact: true }).press("Home");
    await expect(speed.getByRole("radio", { name: "Standard", exact: true })).toBeChecked();
    await expect.poll(() => savedTiers(page)).toEqual(["fast", "ultrafast", "fast", "standard"]);
  });

  test(`${mode} pool speed restores its saved position after a failed save`, async ({ page }, testInfo) => {
    await installTauriMock(page, { mode, locale: "en", populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.evaluate(() => {
      const scope = window as unknown as {
        __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
        __REJECT_SPEED__: () => void;
      };
      const invoke = scope.__TAURI_INTERNALS__.invoke.bind(scope.__TAURI_INTERNALS__);
      scope.__TAURI_INTERNALS__.invoke = (command, args, options) => {
        const action = (args as { input?: { action?: { type?: string } } } | undefined)?.input?.action?.type;
        if (command === "update_local_routing" || (command === "execute_remote_server_action" && action === "set_routing_policy")) {
          return new Promise((_, reject) => {
            scope.__REJECT_SPEED__ = () => reject({ code: "upstream_error", message: "Synthetic routing save failure" });
          });
        }
        return invoke(command, args, options);
      };
    });
    const speed = speedControl(page);
    await speed.getByRole("radio", { name: "Ultrafast", exact: true }).click();
    await expect(speed.getByRole("radio", { name: "Ultrafast", exact: true })).toBeEnabled();
    await expect(speed.getByRole("radio", { name: "Ultrafast", exact: true })).toBeChecked();
    await expect(speed).toHaveAttribute("aria-busy", "true");
    await expect(speed).toHaveAttribute("data-speed-tier", "ultrafast");
    await page.locator(".pool-controls").screenshot({ path: testInfo.outputPath("saving.png") });
    await page.evaluate(() => (window as unknown as { __REJECT_SPEED__: () => void }).__REJECT_SPEED__());
    await expect(speed.getByRole("radio", { name: "Standard", exact: true })).toBeEnabled();
    await expect(speed.getByRole("radio", { name: "Standard", exact: true })).toBeChecked();
    await expect(speed).toHaveAttribute("aria-busy", "false");
  });
}

test("remote pool speed works without optional runtime routing telemetry", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true, remoteFeatures: ["accounts", "sources", "models"] });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const speed = speedControl(page);
  await speed.getByRole("radio", { name: "Ultrafast", exact: true }).click();
  await expect(speed.getByRole("radio", { name: "Ultrafast", exact: true })).toBeEnabled();
  await expect.poll(() => savedTiers(page)).toEqual(["ultrafast"]);
});

test.describe("touch pool speed", () => {
  test.use({ hasTouch: true, viewport: { width: 390, height: 844 } });
  test("all three positions respond to taps", async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).tap();
    const speed = speedControl(page);
    for (const tier of ["Fast", "Ultrafast", "Standard"] as const) {
      const option = speed.getByRole("radio", { name: tier, exact: true });
      await option.tap();
      await expect(option).toBeEnabled();
      await expect(option).toBeChecked();
    }
    expect(await savedTiers(page)).toEqual(["fast", "ultrafast", "standard"]);
  });
});

for (const theme of ["light", "dark"] as const) {
  for (const width of [1160, 840, 390, 360]) {
    test(`Russian pool speed fits ${theme} at ${width}px`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width, height: 844 });
      await installTauriMock(page, { mode: "local", locale: "ru", theme, populated: true });
      await page.goto("/");
      await page.getByRole("button", { name: "Пул", exact: true }).click();
      const speed = page.getByRole("radiogroup", { name: "Скорость запроса" });
      await expect(speed).toBeVisible();
      for (const [name, tier] of [["Обычная", "standard"], ["Быстрая", "fast"], ["Сверхбыстрая", "ultrafast"]] as const) {
        const option = speed.getByRole("radio", { name, exact: true });
        await option.click();
        await expect(option).toBeEnabled();
        await expect(speed).toHaveAttribute("data-speed-tier", tier);
        await page.locator(".pool-controls").screenshot({ path: testInfo.outputPath(`${tier}.png`), animations: "disabled" });
        expect(await speed.locator("button.active span").evaluate((element) => {
          const control = element.closest(".pool-speed-control")!.getBoundingClientRect();
          const text = element.getBoundingClientRect();
          return element.scrollWidth <= element.clientWidth + 1 && text.left >= control.left && text.right <= control.right;
        })).toBe(true);
        expect(await speed.evaluate((element) => {
          const rect = element.getBoundingClientRect();
          return rect.width === 200 && rect.height === 34;
        })).toBe(true);
        expect(await speed.evaluate((element) => {
          const rect = element.getBoundingClientRect();
          const group = element.closest(".pool-member-toolbar")!.getBoundingClientRect();
          return rect.left >= group.left && rect.right <= group.right && rect.left >= 0 && rect.right <= innerWidth;
        })).toBe(true);
      }
      await page.screenshot({ path: testInfo.outputPath("pool.png"), animations: "disabled" });
    });
  }
}

test("pool speed animation respects reduced motion", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const speed = speedControl(page);
  await speed.getByRole("radio", { name: "Ultrafast", exact: true }).click();
  await expect(speed.getByRole("radio", { name: "Ultrafast", exact: true })).toBeEnabled();
  expect(await speed.locator("button.active").evaluate((element) => getComputedStyle(element).transitionDuration)).toBe("0s");
});
