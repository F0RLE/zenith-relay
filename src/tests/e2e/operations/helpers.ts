import { expect, type Locator, type Page } from "../../bun-playwright";

export async function chooseOption(page: Page, scope: Page | Locator, label: string, value: string) {
  await scope.getByRole("button", { name: new RegExp(`^${label.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}:`) }).click();
  await page.locator(`[role="option"][data-value="${value}"]`).click();
}

export async function settleConfirmation(page: Page, accept = true) {
  const dialog = page.getByRole("dialog", { name: "Confirm action" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: accept ? "Confirm" : "Cancel" }).click();
}

export async function openGatewayApi(page: Page) {
  await page.getByRole("button", { name: "Access", exact: true }).click();
  await page.getByRole("tab", { name: /^(Local|Server) API$/ }).click();
}

export async function openGatewayApplication(page: Page) {
  await page.getByRole("button", { name: "Access", exact: true }).click();
  await page.getByRole("tab", { name: "Applications", exact: true }).click();
  await page.getByRole("tab", { name: "ChatGPT", exact: true }).click();
}

export async function connectPoolToChatGPT(page: Page) {
  await page.getByRole("button", { name: "Connect", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Connect an application" });
  await dialog.getByRole("button", { name: "ChatGPT", exact: true }).click();
}
