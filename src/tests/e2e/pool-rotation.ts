import type { Page } from "../bun-playwright";

export async function openPoolRotation(page: Page) {
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
  return page.getByRole("dialog", { name: "Pool rotation", exact: true });
}
