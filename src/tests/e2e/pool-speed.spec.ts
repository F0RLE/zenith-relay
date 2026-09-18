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

for (const mode of ["local", "remote"] as const) {
  test(`${mode} pool speed drags across all modes and saves only on release`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    const speed = page.getByRole("slider", { name: "Request speed" });
    await expect(speed).toHaveValue("0");
    await expect(speed).toHaveAttribute("aria-valuetext", "Standard");
    await expect(page.getByRole("switch", { name: "Request speed" })).toHaveCount(0);
    await expect(page.locator(".pool-speed-control button")).toHaveCount(0);
    const box = (await speed.boundingBox())!;
    const y = box.y + box.height / 2;
    await page.mouse.move(box.x + box.width / 6, y);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width / 2, y, { steps: 8 });
    await expect(speed).toHaveAttribute("aria-valuetext", "Fast");
    expect(await savedTiers(page)).toEqual([]);
    await page.mouse.move(box.x + box.width * 5 / 6, y, { steps: 8 });
    await expect(speed).toHaveAttribute("aria-valuetext", "Ultrafast");
    expect(await savedTiers(page)).toEqual([]);
    await page.mouse.up();
    await expect(speed).toBeEnabled();
    await expect.poll(() => savedTiers(page)).toEqual(["ultrafast"]);

    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await expect(speed).toHaveValue("2");
    await speed.press("ArrowLeft");
    await expect(speed).toBeEnabled();
    await expect(speed).toHaveAttribute("aria-valuetext", "Fast");
    await speed.press("Home");
    await expect(speed).toBeEnabled();
    await expect(speed).toHaveValue("0");
    await expect.poll(() => savedTiers(page)).toEqual(["ultrafast", "fast", "standard"]);
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
    const speed = page.getByRole("slider", { name: "Request speed" });
    await speed.press("End");
    await expect(speed).toBeDisabled();
    await expect(speed).toHaveValue("2");
    await expect(page.locator(".pool-speed-control")).toHaveAttribute("aria-busy", "true");
    await page.locator(".pool-controls").screenshot({ path: testInfo.outputPath("saving.png") });
    await page.evaluate(() => (window as unknown as { __REJECT_SPEED__: () => void }).__REJECT_SPEED__());
    await expect(speed).toBeEnabled();
    await expect(speed).toHaveValue("0");
    await expect(speed).toHaveAttribute("aria-valuetext", "Standard");
    await expect(page.locator(".pool-speed-control")).toHaveAttribute("aria-busy", "false");
  });
}

test("pool speed discards a cancelled drag without changing the runtime", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const speed = page.getByRole("slider", { name: "Request speed" });
  const box = (await speed.boundingBox())!;
  await page.mouse.move(box.x + box.width / 6, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width * 5 / 6, box.y + box.height / 2, { steps: 8 });
  await expect(speed).toHaveValue("2");
  await speed.dispatchEvent("pointercancel");
  await page.mouse.up();
  await expect(speed).toHaveValue("0");
  expect(await savedTiers(page)).toEqual([]);
});

test("remote pool speed works without optional runtime routing telemetry", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true, remoteFeatures: ["accounts", "sources", "models"] });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const speed = page.getByRole("slider", { name: "Request speed" });
  await speed.press("End");
  await expect(speed).toBeEnabled();
  await expect.poll(() => savedTiers(page)).toEqual(["ultrafast"]);
});

test.describe("touch pool speed", () => {
  test.use({ hasTouch: true, viewport: { width: 390, height: 844 } });
  test("all three positions respond to taps", async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).tap();
    const speed = page.getByRole("slider", { name: "Request speed" });
    const box = (await speed.boundingBox())!;
    for (const [position, tier] of [[1, "Fast"], [2, "Ultrafast"], [0, "Standard"]] as const) {
      await page.touchscreen.tap(box.x + box.width * (position + 0.5) / 3, box.y + box.height / 2);
      await expect(speed).toBeEnabled();
      await expect(speed).toHaveAttribute("aria-valuetext", tier);
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
      const speed = page.getByRole("slider", { name: "Скорость запроса" });
      await expect(speed).toBeVisible();
      for (const [key, tier] of [["Home", "standard"], ["ArrowRight", "fast"], ["End", "ultrafast"]]) {
        await speed.press(key);
        await expect(speed).toBeEnabled();
        await expect(page.locator(".pool-speed-control")).toHaveAttribute("data-speed-tier", tier);
        await page.locator(".pool-controls").screenshot({ path: testInfo.outputPath(`${tier}.png`), animations: "disabled" });
        expect(await page.locator(".pool-speed-current").evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
        expect(await page.locator(".pool-speed-control").evaluate((element) => {
          const rect = element.getBoundingClientRect();
          const label = element.querySelector(".pool-speed-current")!.getBoundingClientRect();
          const slider = element.querySelector(".pool-speed-switch")!.getBoundingClientRect();
          return rect.width === 200 && rect.height === 34 && label.right <= slider.left && slider.right <= rect.right;
        })).toBe(true);
        expect(await page.locator(".pool-speed-control").evaluate((element) => {
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
  const speed = page.getByRole("slider", { name: "Request speed" });
  await speed.press("End");
  await expect(speed).toBeEnabled();
  await expect(speed).toHaveAttribute("aria-valuetext", "Ultrafast");
  expect(await page.locator(".pool-speed-label").evaluate((element) => getComputedStyle(element).animationName)).toBe("none");
  expect(await page.locator(".pool-speed-selection").evaluate((element) => getComputedStyle(element).transitionDuration)).toBe("0s");
});
