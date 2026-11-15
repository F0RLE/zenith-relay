import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// Isolated dev fixtures, never a customer account or a server payment.
async function seedHistory(page: Page, mode: "large" | "periods" | "unknown" | "empty" = "large") {
  await page.route(/\/src\/preview\/snapshot\.ts(?:\?.*)?$/, async route => {
    const response = await route.fetch();
    const body = await response.text();
    await route.fulfill({ response, body: `${body}\n{
      const now = Date.parse(previewSnapshot.asOf);
      const base = previewSnapshot.requests.find(row => row.accounting === "posted" && row.billing.postedMicroUsd !== null);
      previewSnapshot.payments = Array.from({ length: ${mode === "empty" ? 0 : 50} }, (_, i) => ({
        id: "history_payment_" + i,
        at: new Date(now - (i + 0.5) * 86400000).toISOString(),
        kind: i % 4 === 0 ? "Возврат платежа" : "Пополнение",
        amount: i % 4 === 0 ? -1000000n : BigInt(i + 1) * 1000000n,
        status: i % 5 === 0 ? "Ожидает оплаты" : i % 4 === 0 ? "Возвращено" : "Зачислено"
      }));
      previewSnapshot.requests = Array.from({ length: ${mode === "large" ? 5000 : mode === "empty" ? 0 : 5} }, (_, i) => ({
        ...base, id: "history_request_" + i,
        startedAt: new Date(now - (${mode === "large" ? "(i + 1) * 1000" : "[1, 26, 72, 240, 1080][i] * 3600000"})).toISOString(),
        model: { ...base.model, id: "history_model_" + i % 50, name: "Модель " + i % 50 },
        key: { ...base.key, id: "history_key_" + i % 100, name: "Ключ " + i % 100 },
        billing: { ...base.billing, postedMicroUsd: ${mode === "large" ? "1000n" : "1000000n"} }
      }));
      if (${JSON.stringify(mode)} === "large") {
        for (const [i, accounting] of [[1, "pending"], [2, "held"]]) {
          previewSnapshot.requests[i].accounting = accounting;
          previewSnapshot.requests[i].billing.postedMicroUsd = null;
        }
      }
      if (${JSON.stringify(mode)} === "unknown") previewSnapshot.requests[0].billing.postedMicroUsd = null;
    }` });
  });
}

async function choose(page: Page, label: string, option: string) {
  await page.getByRole("combobox", { name: label, exact: true }).click();
  await page.getByRole("option", { name: option, exact: true }).click();
}

test("50 payments stay paginated, sorted and filterable", async ({ page }, testInfo) => {
  await seedHistory(page);
  await page.goto("/app/billing");
  const rows = page.locator(".payment-table tbody tr");
  const pager = page.getByRole("navigation", { name: "Страницы платежей", exact: true });
  await expect(rows).toHaveCount(10);
  await expect(rows.first()).toContainText("−1,00");
  const balance = (await page.locator(".billing-layout .balance-panel").boundingBox())!;
  const history = (await page.locator(".billing-layout .payment-panel").boundingBox())!;
  if (history.x > balance.x + balance.width) {
    expect(Math.abs(balance.y + balance.height - history.y - history.height)).toBeLessThan(1);
  }
  await expect(pager.getByRole("status")).toHaveText("1–10 из 50");
  await expect(pager.getByRole("button", { name: "Предыдущая страница", exact: true })).toBeDisabled();
  await pager.getByRole("button", { name: "Следующая страница", exact: true }).click();
  await expect(pager.getByRole("status")).toHaveText("11–20 из 50");
  await expect(rows.first()).toContainText("+11,00");
  for (const size of [25, 50]) {
    await choose(page, "Строк", String(size));
    await expect(rows).toHaveCount(size);
    await expect(pager.getByRole("status")).toHaveText(`1–${size} из 50`);
  }
  await expect(pager.getByRole("button", { name: "Следующая страница", exact: true })).toBeDisabled();
  await choose(page, "Строк", "10");
  await pager.getByRole("button", { name: "Следующая страница", exact: true }).click();
  await choose(page, "Тип платежа", "Возврат платежа");
  await expect(pager.getByRole("status")).toHaveText("1–10 из 13");
  await choose(page, "Период платежей, UTC", "Последние 7 дней");
  await expect(rows).toHaveCount(2);
  await choose(page, "Статус платежа", "Зачислено");
  await expect(page.getByText("Платежей не найдено", { exact: true })).toBeVisible();
  await expect(pager.getByRole("status")).toHaveText("0 записей");
  await page.getByRole("button", { name: "Сбросить фильтры платежей", exact: true }).click();
  await expect(rows).toHaveCount(10);
  await expect(pager.getByRole("status")).toHaveText("1–10 из 50");
  await pager.scrollIntoViewIfNeeded();
  const value = pager.locator(".option-trigger .option-value");
  expect(await value.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  if (testInfo.project.name.startsWith("mobile")) {
    expect((await rows.first().boundingBox())!.height).toBeLessThan(125);
    const controls = await pager.locator(".icon-button").evaluateAll(elements => elements.map(element => element.getBoundingClientRect().top));
    expect(controls[0]).toBe(controls[1]);
  }
  await page.screenshot({ path: testInfo.outputPath("payment-history.png"), fullPage: true });
});

test("5000 requests and 100 key groups paginate without changing totals", async ({ page }, testInfo) => {
  await seedHistory(page);
  await page.goto("/app/usage");
  const pager = page.getByRole("navigation", { name: "Страницы использования", exact: true });
  const requests = page.locator(".request-table tbody tr");
  const groups = page.locator(".aggregate-table tbody tr");
  const totals = page.locator(".usage-metrics");
  await expect(requests).toHaveCount(8);
  await expect(pager.getByRole("status")).toHaveText(/^1–8 из 5\s000$/);
  await expect(requests.nth(1)).toContainText("Ожидает данных");
  await expect(requests.nth(2)).toContainText("Удержание");
  const before = await totals.innerText();
  if (testInfo.project.name.startsWith("mobile")) {
    expect((await requests.first().boundingBox())!.height).toBeLessThan(140);
    await requests.first().getByRole("button", { name: "Детали запроса history_request_0", exact: true }).click();
    await expect(page.getByRole("dialog")).toContainText("Модель 0");
    await page.keyboard.press("Escape");
  }
  await pager.getByRole("button", { name: "Следующая страница", exact: true }).click();
  await expect(pager.getByRole("status")).toHaveText(/^9–16 из 5\s000$/);
  expect(await totals.innerText()).toBe(before);
  await page.getByRole("group", { name: "Представление использования", exact: true }).getByRole("button", { name: "API-ключи", exact: true }).click();
  await expect(groups).toHaveCount(8);
  await expect(pager.getByRole("status")).toHaveText(/^1–8 из 100 · Запросы: 5\s000$/);
  if (testInfo.project.name.startsWith("mobile")) {
    expect((await groups.first().boundingBox())!.height).toBeLessThan(140);
    expect(await groups.first().locator(".group-charge").evaluate(element => element.getClientRects().length)).toBe(1);
  }
  await pager.getByRole("button", { name: "Следующая страница", exact: true }).click();
  await expect(pager.getByRole("status")).toHaveText(/^9–16 из 100 · Запросы: 5\s000$/);
  await groups.first().getByRole("button", { name: "Ключ 8", exact: true }).click();
  await expect(page.getByRole("combobox", { name: "API-ключ", exact: true })).toHaveText("Ключ 8");
  await expect(pager.getByRole("status")).toHaveText("1–8 из 50");
  await choose(page, "API-ключ", "Все ключи");
  await page.getByRole("group", { name: "Представление использования", exact: true }).getByRole("button", { name: "Модели", exact: true }).click();
  await expect(groups).toHaveCount(8);
  await expect(pager.getByRole("status")).toHaveText(/^1–8 из 50 · Запросы: 5\s000$/);
  await choose(page, "Строк", "32");
  await expect(groups).toHaveCount(32);
  await pager.getByRole("button", { name: "Следующая страница", exact: true }).click();
  await expect(groups).toHaveCount(18);
  await expect(pager.getByRole("status")).toHaveText(/^33–50 из 50 · Запросы: 5\s000$/);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("grouped-history.png"), fullPage: true });
});

test("Balance has no spending block while Usage retains rolling periods", async ({ page }) => {
  await seedHistory(page, "periods");
  await page.goto("/app/billing");
  await expect(page.getByRole("region", { name: "Расходы", exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Расход по запросам", exact: true })).toHaveCount(0);
  await page.goto("/app/usage");
  const charge = page.locator(".usage-metric").filter({ has: page.getByText("Списано", { exact: true }) }).locator("strong");
  for (const [label, expected] of [["Последние 24 часа", "1,000000 USD"], ["Последние 7 дней", "3,000000 USD"], ["Последние 30 дней", "4,000000 USD"]]) {
    await choose(page, "Период, UTC", label!);
    await expect(charge).toHaveText(expected!);
  }
});

test("unknown charges and empty histories are not presented as known charges", async ({ page }) => {
  await seedHistory(page, "unknown");
  await page.goto("/app/usage");
  const charge = page.locator(".usage-metric").filter({ has: page.getByText("Списано", { exact: true }) });
  await expect(charge.locator("strong")).toHaveText("— USD");
  await expect(charge).toHaveAttribute("data-relay-tooltip", "Часть итоговых сумм недоступна");
  await page.unroute(/\/src\/preview\/snapshot\.ts(?:\?.*)?$/);
  await seedHistory(page, "empty");
  await page.goto("/app/billing");
  await expect(page.getByText("Платежей пока нет", { exact: true })).toBeVisible();
  await expect(page.getByRole("navigation", { name: "Страницы платежей", exact: true }).getByRole("status")).toHaveText("0 записей");
  await page.goto("/app/usage");
  await expect(charge.locator("strong")).toHaveText("0,000000 USD");
});
