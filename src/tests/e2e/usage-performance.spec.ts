import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

const searchedRequests = (page: Page) => page.evaluate(() => (window as unknown as {
  __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { requestIdQuery?: string } } }>;
}).__TAURI_TEST_INVOKES__
  .filter(({ command, args }) => command === "get_local_usage_page" && args.input?.requestIdQuery)
  .map(({ args }) => args.input!.requestIdQuery));

test("typing a request ID issues one settled search instead of one query per character", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, { locale: "en" });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await page.getByRole("button", { name: "More filters", exact: true }).click();
  const input = page.getByRole("textbox", { name: "Request ID", exact: true });
  await input.pressSequentially("req_synthetic_local", { delay: 5 });
  await expect(input).toHaveValue("req_synthetic_local");
  await page.clock.fastForward(300);
  await expect.poll(() => searchedRequests(page)).toEqual(["req_synthetic_local"]);
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
  await page.clock.fastForward(500);
  expect(await searchedRequests(page)).toEqual(["req_synthetic_local"]);
});

test("clearing unfinished search cancels its timer and keeps the current report", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, { locale: "en" });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await page.getByRole("button", { name: "More filters", exact: true }).click();
  const input = page.getByRole("textbox", { name: "Request ID", exact: true });
  await input.fill("unfinished");
  await input.fill("");
  await page.clock.fastForward(1_000);
  expect(await searchedRequests(page)).toEqual([]);
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
});

test("leaving the request tab cancels a pending search without filtering aggregates", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, { locale: "en" });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await page.getByRole("button", { name: "More filters", exact: true }).click();
  await page.getByRole("textbox", { name: "Request ID", exact: true }).fill("req_synthetic_local");
  await page.getByRole("tab", { name: "Models", exact: true }).click();
  await page.clock.fastForward(1_000);
  expect(await searchedRequests(page)).toEqual([]);
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await page.getByRole("tab", { name: "Sends", exact: true }).click();
  await page.clock.fastForward(300);
  await expect.poll(() => searchedRequests(page)).toEqual(["req_synthetic_local"]);
});
