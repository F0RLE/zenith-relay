import { expect } from "@playwright/test";
import { join } from "node:path";

export async function checkModelStatus({ page, foreign, owner, origin, api, model }) {
  const endpoint = api + "models/status";
  const customer = page.context().request;
  const read = async (client, query = "") => {
    const r = await client.get(endpoint + query);
    expect(r.status()).toBe(200);
    expect(r.headers()["cache-control"]).toBe("no-store");
    return (await r.json()).data;
  };
  expect((await fetch(endpoint)).status).toBe(401);
  expect((await owner.context().request.get(endpoint)).status()).toBe(401);
  for (const q of ["?hours=720", "?hours=24&hours=168", "?workspace_id=" + crypto.randomUUID()])
    expect((await customer.get(endpoint + q)).status()).toBe(400);
  const [day, week] = await Promise.all([read(customer), read(foreign.context().request, "?hours=168")]);
  for (const [data, n, hours] of [[day, 48, 24], [week, 28, 168]]) {
    expect(data.hours).toBe(hours);
    expect(data.as_of_ms - data.intervals[0].from_ms).toBe(hours * 3_600_000);
    expect(data.intervals.at(-1).to_ms).toBe(data.as_of_ms);
    expect(data.items.map(r => r.model)).toEqual([model]);
    expect(data.items[0].history).toHaveLength(n);
    expect(data.items[0].history.every(b => b.state === "unknown" && b.monitored_ms === 0)).toBe(true);
    expect(data.summary.requests).toBe(0);
    expect(data.items[0].history[0].requests).toBeNull();
  }

  await page.goto(origin + "/app/status");
  await expect(page.getByRole("heading", { name: "Status", exact: true })).toBeVisible();
  const history = page.getByRole("group", { name: "Availability history: " + model });
  const refresh = page.getByRole("button", { name: "Refresh model status" });
  const tooltip = page.getByRole("tooltip");
  await expect(history.locator(":scope > span")).toHaveCount(48);
  for (const [width, theme] of [[320, "dark"], [1120, "light"]]) {
    await page.setViewportSize({ width, height: 760 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
    await page.reload();
    await expect(page.locator('.hosted-app[data-workspace="customer"]')).toHaveAttribute("data-theme", theme);
    await page.getByRole("button", { name: "7 days" }).click();
    await expect(history.locator(":scope > span")).toHaveCount(28);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    if (process.env.RELAY_TEST_STATUS_CAPTURE)
      await page.screenshot({ path: join(process.env.RELAY_TEST_STATUS_CAPTURE, `status-${width}.png`), fullPage: true });
    await history.scrollIntoViewIfNeeded();
    await history.focus();
    await history.press("Home");
    await expect(tooltip).toBeVisible();
    await expect(tooltip).toContainText("UTC");
    expect(await tooltip.getByText("No data", { exact: true }).count()).toBe(4);
    const box = await tooltip.boundingBox();
    expect(box.x >= 7 && box.x + box.width <= width - 7 && box.y >= 7 && box.y + box.height <= 753).toBe(true);
    if (process.env.RELAY_TEST_STATUS_CAPTURE)
      await page.screenshot({ path: join(process.env.RELAY_TEST_STATUS_CAPTURE, `interval-${width}.png`) });
    await history.press("End");
    await expect(tooltip.locator("dd").nth(2)).toContainText("0");
    await history.press("Escape");
    await expect(tooltip).toHaveCount(0);
  }
  const search = page.getByRole("searchbox", { name: "Search models" });
  await search.fill("not-in-catalog");
  await expect(page.getByText("No matching models")).toBeVisible();
  await search.fill("");
  await expect(history).toBeVisible();

  const failing = endpoint + "?hours=168";
  let release;
  const gate = new Promise(resolve => { release = resolve; });
  await page.route(failing, async route => {
    await gate;
    await route.fulfill({ status: 503, json: { error: { code: "unavailable" } } });
  });
  try {
    await refresh.click();
    await expect(page.getByRole("status")).toHaveText("Connecting…");
    await expect(history).toHaveCount(0);
    release();
    await expect(page.getByRole("alert")).toHaveText(/Cannot connect/);
    await expect(page.getByText("No models available yet")).toHaveCount(0);
  } finally {
    release();
    await page.unroute(failing);
  }
  await page.getByRole("button", { name: "Retry" }).click();
  await expect(history).toBeVisible();
}
