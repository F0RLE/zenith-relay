import { expect } from "@playwright/test";
import { join } from "node:path";
import { fixtureExists, readFixture, writeFixture } from "../../test-support/files.mjs";

export async function checkDesktopPoolReview(page, exchange, source, name, model) {
  const wait = file => expect.poll(() => fixtureExists(exchange, file), { timeout: 15000 }).toBe(true);
  await page.getByRole("button", { name: "Desktop", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Desktop" });
  await dialog.getByText("Pool review", { exact: true }).click();
  writeFixture(exchange, "desktop-pool-ready", source);
  const row = page.getByRole("article", { name, exact: true });
  for (const [action, label] of [["include", "Add to pool"], ["exclude", "Remove from pool"]]) {
    await wait(`desktop-pool-${action}`);
    await dialog.getByLabel("Pool review code from Desktop").fill(readFixture(exchange, `desktop-pool-${action}`));
    await dialog.getByRole("button", { name: "Review source" }).click();
    await expect(dialog.getByText(name, { exact: true })).toBeVisible();
    await expect(dialog.getByText("Device: Copy fixture")).toBeVisible();
    await expect(dialog.getByText(model, { exact: false })).toBeVisible();
    const approve = dialog.getByRole("button", { name: label, exact: true });
    await expect(approve).toBeFocused();
    if (action === "include") {
      await page.setViewportSize({ width: 320, height: 760 });
      await approve.scrollIntoViewIfNeeded();
      expect(await dialog.evaluate(n => n.scrollWidth <= n.clientWidth && n.getBoundingClientRect().right <= innerWidth)).toBe(true);
      if (process.env.RELAY_TEST_SOURCE_INTAKE_CAPTURE)
        await page.screenshot({ path: join(process.env.RELAY_TEST_SOURCE_INTAKE_CAPTURE, "desktop-pool-review-320.png") });
    }
    await approve.click();
    await expect(dialog.getByText("Pool membership updated.")).toBeVisible();
    writeFixture(exchange, `desktop-pool-${action}-done`);
    if (action === "include") await dialog.getByRole("button", { name: "Clear review" }).click();
  }
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await expect(row.getByText(/Outside pool/)).toBeVisible();
  await expect(row.getByText("Proxy assigned", { exact: true })).toBeVisible();
}
