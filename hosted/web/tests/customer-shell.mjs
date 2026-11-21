import { expect } from "@playwright/test";
import { join } from "node:path";

export const balanceLink = page => page.locator(".hosted-tools .hosted-balance");
const sections = page => page.getByRole("navigation", { name: "Sections", exact: true });
const menu = page => page.getByRole("button", { name: "Menu", exact: true });

export async function openCustomerMenu(page) {
  await expect(balanceLink(page)).toBeVisible();
  if (await menu(page).isVisible()) await menu(page).click();
}

export async function navigateCustomer(page, name) {
  await openCustomerMenu(page);
  await sections(page).getByRole("link", { name, exact: true }).click();
}

export async function setWorkspaceLanguage(page, locale) {
  await page.locator(".language-roller-trigger").click();
  await page.getByRole("listbox").press(locale[0]);
  await page.keyboard.press("Enter");
  await expect(page.locator(".hosted-app")).toHaveAttribute("lang", locale);
}

export async function checkCustomerShell({ mobile, desktop, origin }) {
  const shell = mobile.locator('.hosted-app[data-workspace="customer"]');
  await expect(shell).toBeVisible();
  await expect(menu(mobile)).toHaveAttribute("aria-expanded", "false");
  await menu(mobile).press("Enter");
  const drawer = mobile.getByRole("dialog", { name: "Navigation", exact: true });
  await expect(drawer).toBeVisible();
  await expect(sections(mobile).getByRole("link")).toHaveText(["Overview", "Usage", "Models", "Status", "API keys", "Balance"]);
  await expect(drawer.getByRole("link", { name: "Overview" })).toHaveAttribute("aria-current", "page");
  await expect(drawer.locator(".sidebar-account-nav").getByRole("link")).toHaveText(["API keys", "Balance"]);
  await expect(drawer.getByRole("button", { name: "Close menu" }).first()).toBeFocused();
  await mobile.keyboard.press("Shift+Tab");
  await expect(drawer.getByRole("button", { name: "Sign out" })).toBeFocused();
  await mobile.keyboard.press("Escape");
  await expect(menu(mobile)).toBeFocused();
  await expect(drawer).toHaveCount(0);

  const viewport = desktop.viewportSize();
  await desktop.setViewportSize({ width: 940, height: 760 });
  await desktop.getByRole("button", { name: "Expand menu" }).click();
  await navigateCustomer(desktop, "Models");
  await expect(desktop.getByRole("button", { name: "Collapse menu" })).toBeVisible();
  await expect(sections(desktop).getByRole("link", { name: "Models" })).toHaveAttribute("aria-current", "page");
  const heading = desktop.getByRole("heading", { name: "Models and pricing" });
  await expect(heading).toBeVisible();
  await desktop.setViewportSize({ width: 640, height: 760 });
  const tools = desktop.locator(".hosted-tools");
  await expect(tools).toHaveCSS("position", "sticky");
  expect((await heading.boundingBox()).y).toBeGreaterThanOrEqual((await tools.boundingBox()).height);
  expect(await desktop.locator(".relay-content").evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
  const capture = process.env.RELAY_TEST_CATALOG_CAPTURE;
  if (capture) await desktop.screenshot({ path: join(capture, "customer-640-expanded.png") });
  await desktop.setViewportSize({ width: 940, height: 760 });
  await desktop.getByRole("button", { name: "Collapse menu" }).click();
  await navigateCustomer(desktop, "Overview");
  await expect(desktop.getByRole("button", { name: "Expand menu" })).toBeVisible();
  await expect(desktop).toHaveURL(url => url.origin === origin && url.pathname === "/app/overview");
  if (viewport) await desktop.setViewportSize(viewport);

  const theme = mobile.locator("[data-theme-toggle]");
  const before = await shell.getAttribute("data-theme");
  const changed = before === "light" ? "dark" : "light";
  await theme.click();
  await expect(shell).toHaveAttribute("data-theme", changed);
  await navigateCustomer(mobile, "Models");
  await expect(shell).toHaveAttribute("data-theme", changed);
  await theme.click();
  await expect(shell).toHaveAttribute("data-theme", before);
  await navigateCustomer(mobile, "Overview");
  await mobile.evaluate(() => localStorage.removeItem("relay-hosted-theme"));

  if (capture) {
    await mobile.setViewportSize({ width: 320, height: 760 });
    await openCustomerMenu(mobile);
    await expect(drawer).toBeVisible();
    await mobile.screenshot({ path: join(capture, "customer-drawer-320.png") });
    await mobile.keyboard.press("Escape");
    await mobile.setViewportSize({ width: 360, height: 760 });
  }
}
