import { expect } from "@playwright/test";
import { join } from "node:path";

export async function checkOwnerOverview({ owner, customer, origin, api }) {
  const endpoint = api + "admin/overview/usage", url = origin + "/admin/overview";
  const http = owner.context().request;
  const report = owner.locator(".owner-overview");
  const charges = name => report.getByRole("article", { name, exact: true });
  const total = charges("API charges").locator(".overview-chart-summary");
  const period = name => report.getByRole("button", { name, exact: true });
  const refresh = period("Refresh usage");
  const read = async query => {
    const response = await http.get(endpoint + query);
    expect(response.status()).toBe(200);
    expect(response.headers()["cache-control"]).toBe("no-store");
    return (await response.json()).data;
  };
  expect((await customer.context().request.get(endpoint)).status()).toBe(401);
  expect((await fetch(endpoint)).status).toBe(401);
  for (const query of ["?hours=1", "?hours=24&hours=0", "?model=bad%20model", "?workspace_id=other"])
    expect((await http.get(endpoint + query)).status()).toBe(400);

  await owner.goto(url);
  await expect(total).toHaveText("1.204000 USD");
  const initial = await read("");
  expect(initial.totals.spend_micro_usd).toBe("1204000");
  // One verified charge, two cap refusals, a sent/queued pair closed on recovery, and one partial charge.
  expect(initial.totals.requests).toBe("6");
  expect(initial.totals.total_tokens).toBe("100000");
  expect(initial.as_of_ms - initial.from_ms).toBe(86400000);
  expect((await read("?model=fixture-text")).totals).toEqual(initial.totals);
  expect((await read("?model=Fixture-text")).totals.requests).toBe("0");
  await expect(report.getByRole("table", { name: "Chart data" }).getByRole("row")).toHaveCount(25);

  for (const [width, theme, locale, label] of [
    [320, "dark", "ru", "Списано за API"], [320, "light", "zh-CN", "API 扣费"], [1120, "light", "en", "API charges"],
  ]) {
    await owner.setViewportSize({ width, height: 780 });
    await owner.emulateMedia({ colorScheme: theme === "light" ? "dark" : "light" });
    await owner.evaluate(([theme, locale]) => {
      localStorage.setItem("relay-hosted-theme", theme);
      localStorage.setItem("relay-hosted-locale", locale);
    }, [theme, locale]);
    await owner.reload();
    await expect(owner.locator(".hosted-entry")).toHaveAttribute("data-theme", theme);
    await expect(owner.locator(".hosted-entry")).toHaveCSS("background-color", theme === "light" ? "rgb(244, 243, 238)" : "rgb(20, 20, 20)");
    await expect(charges(label)).toBeVisible();
    expect(await report.evaluate(el => document.documentElement.scrollWidth <= innerWidth &&
      [...el.querySelectorAll("article, button")].every(element => {
        const rect = element.getBoundingClientRect();
        return rect.left >= 0 && rect.right <= innerWidth;
      }))).toBe(true);
    if (width === 320) expect(await report.evaluate(el => {
      const time = el.querySelector("time").getBoundingClientRect();
      const button = el.querySelector(".report-refresh").getBoundingClientRect();
      return time.top >= button.top - 1 && time.bottom <= button.bottom + 1;
    })).toBe(true);
    if (process.env.RELAY_TEST_HISTORY_UI_CAPTURE)
      await owner.screenshot({ path: join(process.env.RELAY_TEST_HISTORY_UI_CAPTURE, `owner-overview-${width}-${locale}-${theme}.png`), fullPage: true });
  }

  for (const [hours, label, rows] of [[168, "7 days", 8], [720, "30 days", 31], [0, "All time", 31]]) {
    const requested = owner.waitForResponse(endpoint + "?hours=" + hours);
    await period(label).click();
    const response = await requested;
    expect(response.request().method()).toBe("GET");
    expect(response.status()).toBe(200);
    await expect(period(label)).toHaveAttribute("aria-pressed", "true");
    await expect(report.getByRole("table", { name: "Chart data" }).getByRole("row")).toHaveCount(rows);
    await expect(total).toHaveText("1.204000 USD");
  }
  const release = Promise.withResolvers(), finished = Promise.withResolvers();
  await owner.route(endpoint + "?hours=168", async route => {
    try {
      const slow = await route.fetch();
      await release.promise;
      await route.fulfill({ response: slow }).catch(() => { /* The old read can already be aborted. */ });
    } finally { finished.resolve(); }
  }, { times: 1 });
  try {
    const started = owner.waitForRequest(endpoint + "?hours=168");
    await period("7 days").click();
    await started;
    await period("All time").click();
    await expect(total).toHaveText("1.204000 USD");
  } finally { release.resolve(); }
  await finished.promise;
  await expect(report.getByRole("table", { name: "Chart data" }).getByRole("row")).toHaveCount(31);

  const plot = charges("API charges").getByRole("link", { name: "API charges: time chart", exact: true });
  await plot.focus();
  await plot.press("Home");
  await expect(charges("API charges").getByRole("tooltip")).toBeVisible();
  await plot.press("Escape");
  await expect(charges("API charges").getByRole("tooltip")).toHaveCount(0);

  await owner.route(endpoint + "?hours=0", route => route.fulfill({
    status: 503, json: { error: { code: "admin_unavailable" } },
  }), { times: 1 });
  await refresh.click();
  await expect(report.getByRole("alert")).toBeVisible();
  await expect(total).toHaveCount(0);
  await period("Retry").click();
  await expect(total).toHaveText("1.204000 USD");

  return async () => {
    await refresh.click();
    await expect(total).toHaveText("1.200600 USD");
    await expect(period("All time")).toHaveAttribute("aria-pressed", "true");
    const after = await read("?hours=0");
    expect(after.totals.requests).toBe(initial.totals.requests);
    expect(after.totals.total_tokens).toBe("100020");
    expect(after.totals.spend_micro_usd).toBe("1200600");
  };
}
