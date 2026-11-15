import { expect, test } from "@playwright/test";

test("inline presets and custom input do not credit the wallet before submission", async ({ page }, testInfo) => {
  await page.goto("/app/billing");
  const amount = page.getByRole("spinbutton", { name: "Сумма, USD", exact: true });
  await expect(page.getByText(/Зарезервировано под запросы/)).toHaveCount(0);
  await expect(page.locator(".balance-panel > p")).toHaveCount(0);
  await expect(amount).toHaveCSS("appearance", "textfield");
  await expect(amount).toHaveAttribute("min", "1");
  await expect(amount).toHaveAttribute("max", "1000");
  await expect(amount).toHaveAttribute("step", "1");
  for (const preset of [10, 25, 50, 100]) {
    const button = page.getByRole("button", { name: `${preset} USD`, exact: true });
    await button.click();
    await expect(amount).toHaveValue(String(preset));
    await expect(button).toHaveAttribute("aria-pressed", "true");
    await expect(page.locator('.topup-presets [aria-pressed="true"]')).toHaveCount(1);
  }
  await amount.fill("37");
  await amount.press("ArrowUp");
  await expect(amount).toHaveValue("38");
  await amount.press("ArrowDown");
  await expect(amount).toHaveValue("37");
  await expect(page.locator('.topup-presets [aria-pressed="true"]')).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Баланс: 42,80 USD", exact: true })).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  const form = page.getByRole("form", { name: "Пополнение баланса", exact: true });
  await expect(page.getByText("Демо-пополнение, без списания денег.", { exact: true })).toHaveCount(0);
  await expect(page.getByText("Списания по запросам и API-ключам — во вкладке «Использование».", { exact: true })).toHaveCount(0);
  await expect(page.locator(".payment-panel > header")).toHaveText("История платежей");
  await expect(form).not.toHaveAttribute("aria-describedby");
  expect(await form.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  const balance = (await page.locator(".billing-layout .balance-panel").boundingBox())!;
  const history = (await page.locator(".billing-layout .payment-panel").boundingBox())!;
  if (history.x > balance.x + balance.width) {
    expect(Math.abs(balance.y - history.y)).toBeLessThan(1);
    expect(Math.abs(balance.y + balance.height - history.y - history.height)).toBeLessThan(1);
  } else {
    expect(history.y).toBeGreaterThanOrEqual(balance.y + balance.height);
  }
  await amount.hover();
  await expect(amount).toBeFocused();
  await page.locator(".balance-panel").screenshot({ path: testInfo.outputPath("balance-card-no-reserve-or-spinners.png") });
  await page.screenshot({ path: testInfo.outputPath("inline-topup.png"), fullPage: true });
});

for (const method of ["Банковская карта", "Криптовалюта", "Банковский перевод"]) test(`inline ${method} selection records only a local demo credit`, async ({ page }) => {
  const apiCalls: string[] = [];
  page.on("request", request => { if (["fetch", "xhr"].includes(request.resourceType())) apiCalls.push(request.url()); });
  await page.goto("/app/billing");
  await page.getByRole("combobox", { name: "Способ оплаты", exact: true }).click();
  await page.getByRole("option", { name: method, exact: true }).click();
  await page.getByRole("spinbutton", { name: "Сумма, USD", exact: true }).fill("37");
  const submit = page.getByRole("button", { name: "Пополнить", exact: true });
  await submit.click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.locator(".topup-receipt")).toContainText(`Добавлено 37,00 USD. ${method}.`);
  await expect(page.getByRole("button", { name: "Баланс: 79,80 USD", exact: true })).toBeVisible();
  await expect(submit).toBeDisabled();
  await expect(page.getByRole("spinbutton", { name: "Сумма, USD", exact: true })).toHaveValue("");
  const operations = page.getByRole("region", { name: "Платёжные операции", exact: true });
  await expect(operations.locator("tbody tr")).toHaveCount(4);
  const credit = operations.locator("tbody tr").first();
  await expect(credit).toContainText("+37,00");
  await credit.getByRole("button", { name: "Пополнение", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Платёжная операция", exact: true })).toContainText(method);
  await expect(page.getByRole("dialog")).not.toContainText(/демо|вымышл|макет|до перезагрузки/iu);
  await page.reload();
  await expect(page.getByRole("button", { name: "Баланс: 42,80 USD", exact: true })).toBeVisible();
  await expect(operations.locator("tbody tr")).toHaveCount(3);
  await expect(operations).not.toContainText("+37,00");
  expect(apiCalls).toEqual([]);
});

test("invalid amounts and missing payment selection cannot credit the wallet", async ({ page }) => {
  await page.goto("/app/billing");
  const amount = page.getByRole("spinbutton", { name: "Сумма, USD", exact: true });
  const submit = page.getByRole("button", { name: "Пополнить", exact: true });
  for (const value of ["0", "-1", "1.5", "1001"]) {
    await amount.fill(value);
    await submit.click();
    await expect(page.getByRole("alert")).toHaveText("Введите целую сумму от 1 до 1 000 USD.");
    await expect(amount).toHaveAttribute("aria-invalid", "true");
    await expect(amount).toBeFocused();
    await expect(page.getByRole("button", { name: "Баланс: 42,80 USD", exact: true })).toBeVisible();
  }
  await amount.fill("");
  await expect(submit).toBeDisabled();
  await page.getByRole("button", { name: "10 USD", exact: true }).click();
  await expect(page.getByRole("alert")).toHaveCount(0);
  const method = page.getByRole("combobox", { name: "Способ оплаты", exact: true });
  await method.click();
  await page.getByRole("option", { name: "Выберите способ", exact: true }).click();
  await submit.click();
  await expect(page.getByRole("alert")).toHaveText("Выберите способ оплаты.");
  await expect(page.getByRole("button", { name: "Баланс: 42,80 USD", exact: true })).toBeVisible();
  await method.focus();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("End");
  await expect(page.getByRole("option", { name: "Банковский перевод", exact: true })).toHaveAttribute("data-active", "true");
  await page.keyboard.press("Enter");
  await expect(method).toContainText("Банковский перевод");
  await amount.focus();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("button", { name: "Баланс: 52,80 USD", exact: true })).toBeVisible();
});
