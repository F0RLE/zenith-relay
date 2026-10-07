import { expect, test } from "../../bun-playwright";
import { emitTauriEvent, installTauriMock } from "../tauri-mock";

test("macOS leaves space for system buttons without drawing duplicates", async ({ page }) => {
  await installTauriMock(page, { platform: "macos" });
  await page.goto("/");
  const titlebar = page.locator(".titlebar-macos");
  await expect(titlebar).toBeVisible();
  await expect(titlebar.getByRole("button")).toHaveCount(0);
  for (const width of [1160, 1440]) {
    await page.setViewportSize({ width, height: 900 });
    const logo = titlebar.locator(".titlebar-logo");
    const normal = await logo.boundingBox();
    expect(normal!.x).toBeGreaterThan(66);
    await emitTauriEvent(page, "tauri://resize", { width, height: 900, fullscreen: true });
    await expect(titlebar).toHaveClass(/titlebar-fullscreen/);
    await expect(titlebar.locator(".titlebar-native-controls")).toBeHidden();
    expect((await logo.boundingBox())!.x).toBeLessThan(30);
    await emitTauriEvent(page, "tauri://resize", { width, height: 900, fullscreen: false });
    await expect(titlebar).not.toHaveClass(/titlebar-fullscreen/);
    expect((await logo.boundingBox())!.x).toBeCloseTo(normal!.x, 1);
  }
});

for (const platform of ["windows", "linux"] as const) {
  test(`${platform} window buttons use the expected order and actions`, async ({ page }) => {
    await installTauriMock(page, { platform });
    await page.goto("/");
    const controls = page.locator(`.window-controls-${platform}`);
    expect(await controls.getByRole("button").evaluateAll((items) => items.map((item) => item.className)))
      .toEqual(["minimize", "maximize", "close"]);
    await controls.getByRole("button", { name: "Close", exact: true }).click();
    await controls.getByRole("button", { name: "Minimize", exact: true }).click();
    await controls.getByRole("button", { name: "Maximize", exact: true }).click();
    await expect.poll(async () => page.evaluate(() =>
      (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__
        .map(({ command }) => command).filter((command) => /^plugin:window\|(close|minimize|toggle_maximize)$/.test(command))
    )).toEqual(["plugin:window|close", "plugin:window|minimize", "plugin:window|toggle_maximize"]);
  });
}
