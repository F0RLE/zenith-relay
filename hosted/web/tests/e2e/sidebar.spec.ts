import { expect, test } from "@playwright/test";

test("logo toggles the rail or closes the drawer without navigating", async ({ page }) => {
  await page.goto("/app/usage");
  const shell = page.locator(".relay-shell");
  const sidebar = page.locator(".relay-sidebar");
  const logo = sidebar.locator(".hosted-mode");
  const mobile = page.viewportSize()!.width < 640;
  if (mobile) await page.getByRole("button", { name: "Меню", exact: true }).click();
  // Opening the drawer places the logo under the old menu-button coordinates.
  // Move the mouse away before checking the resting, rather than hovered, fill.
  await page.mouse.move(page.viewportSize()!.width - 1, 0);
  await expect(logo).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  await expect(sidebar.locator(".sidebar-version, .sidebar-collapse")).toHaveCount(0);
  await expect(sidebar).not.toContainText(/Макет|без сервера|Подключение и помощь/);
  await expect(sidebar.locator(".sidebar-footer").getByRole("button", { name: "Выйти", exact: true })).toBeVisible();
  if (mobile) {
    const logoBounds = await logo.boundingBox();
    const closeBounds = await sidebar.getByRole("button", { name: "Закрыть меню", exact: true }).boundingBox();
    expect(logoBounds!.x + logoBounds!.width + 8).toBeLessThanOrEqual(closeBounds!.x);
    await logo.click();
    await expect(sidebar).not.toBeVisible();
    await expect(page.getByRole("button", { name: "Меню", exact: true })).toBeFocused();
    await expect(page.locator(".relay-content")).not.toHaveAttribute("inert", "");
  } else {
    const expanded = await logo.getAttribute("aria-expanded") === "true";
    await logo.focus();
    await page.keyboard.press("Enter");
    await expect(logo).toHaveAttribute("aria-expanded", String(!expanded));
    if (expanded) await expect(shell).toHaveClass(/sidebar-collapsed/);
    else await expect(shell).not.toHaveClass(/sidebar-collapsed/);
    await expect(logo).toBeFocused();
    await page.keyboard.press("Space");
    await expect(logo).toHaveAttribute("aria-expanded", String(expanded));
    await logo.click();
    await expect(logo).toHaveAttribute("aria-expanded", String(!expanded));
  }
  await expect(page).toHaveURL(/\/app\/usage$/);
  await expect(page.getByRole("heading", { name: "Использование", exact: true })).toBeVisible();
});

test("keys and balance stay at the bottom above the logout divider", async ({ page }, testInfo) => {
  await page.goto("/app/overview");
  const sidebar = page.locator(".relay-sidebar");
  const mobile = page.viewportSize()!.width < 640;
  if (mobile) await page.getByRole("button", { name: "Меню", exact: true }).click();
  const main = sidebar.locator(".sidebar-main-nav > .sidebar-nav-group:not(.sidebar-account-nav)");
  const account = sidebar.locator(".sidebar-account-nav");
  await expect(main.getByRole("button")).toHaveText(["Обзор", "Использование", "Модели", "Статус"]);
  await expect(account.getByRole("button")).toHaveText(["API-ключи", "Баланс"]);
  const mainBox = (await main.boundingBox())!;
  const accountBox = (await account.boundingBox())!;
  const footerBox = (await sidebar.locator(".sidebar-footer").boundingBox())!;
  expect(accountBox.y - mainBox.y - mainBox.height).toBeGreaterThan(32);
  expect(accountBox.y + accountBox.height).toBeLessThanOrEqual(footerBox.y);
  expect(footerBox.y - accountBox.y - accountBox.height).toBeLessThan(32);
  await sidebar.screenshot({ path: testInfo.outputPath("sidebar-bottom-account.png") });
  await account.getByRole("button", { name: "API-ключи", exact: true }).click();
  await expect(page).toHaveURL(/\/app\/keys$/);
  if (mobile) await page.getByRole("button", { name: "Меню", exact: true }).click();
  await expect(account.getByRole("button", { name: "API-ключи", exact: true })).toHaveAttribute("aria-current", "page");
  await account.getByRole("button", { name: "Баланс", exact: true }).click();
  await expect(page).toHaveURL(/\/app\/billing$/);
  if (mobile) await page.getByRole("button", { name: "Меню", exact: true }).click();
  await expect(account.getByRole("button", { name: "Баланс", exact: true })).toHaveAttribute("aria-current", "page");
  await expect(sidebar.locator(".sidebar-footer").getByRole("button", { name: "Выйти", exact: true })).toBeVisible();
});

test("footer logout remains reachable and supports cancelling and confirming", async ({ page }) => {
  await page.goto("/app/keys");
  const sidebar = page.locator(".relay-sidebar");
  if (page.viewportSize()!.width < 640) await page.getByRole("button", { name: "Меню", exact: true }).click();
  else if (await sidebar.locator(".hosted-mode").getAttribute("aria-expanded") === "true") await sidebar.locator(".hosted-mode").click();
  const logout = sidebar.locator(".sidebar-footer").getByRole("button", { name: "Выйти", exact: true });
  await logout.click();
  const dialog = page.getByRole("dialog", { name: "Выйти из аккаунта?", exact: true });
  await dialog.getByRole("button", { name: "Остаться", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(logout).toBeFocused();
  await expect(page).toHaveURL(/\/app\/keys$/);
  await logout.click();
  await dialog.getByRole("button", { name: "Выйти", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(page.locator(".demo-signed-out")).toBeVisible();
});

test("removed help URLs render not found instead of the deleted page", async ({ page }) => {
  for (const url of ["/app/help", "/preview.html?page=help"]) {
    await page.goto(url);
    await expect(page.getByRole("heading", { name: "Страница не найдена", exact: true })).toBeVisible();
    await expect(page.locator(".help-layout, .help-guide")).toHaveCount(0);
  }
});
