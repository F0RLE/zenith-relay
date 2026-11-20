import { expect } from "@playwright/test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { checkOwnerOverview } from "./owner-overview-flow.mjs";
import { checkOwnerUsage } from "./owner-usage-flow.mjs";

export async function checkUsageReport({ a, b, owner, origin, api, wait, issue, exchange }) {
  const client = a.context().request, foreign = b.context().request;
  const read = async path => {
    const r = await client.get(api + path);
    expect(r.status()).toBe(200);
    expect(r.headers()["cache-control"]).toBe("no-store");
    return (await r.json()).data;
  };
  const csv = async path => {
    const r = await client.get(api + path);
    expect(r.status()).toBe(200);
    expect(r.headers()["content-type"]).toBe("text/csv; charset=utf-8");
    expect(r.headers()["content-disposition"]).toBe('attachment; filename="relay-usage.csv"');
    expect(r.headers()["cache-control"]).toBe("no-store");
    return r.text();
  };
  function csvRows(text) {
    // Fixture identifiers and machine values have no CSV delimiters.
    const [header, ...rows] = text.trimEnd().split("\r\n").map(line => line.split(","));
    return rows.map(row => Object.fromEntries(header.map((column, i) => [column, row[i]])));
  }
  const captured = () => a.waitForResponse(r => r.url() === api + "usage/reports" && r.request().method() === "POST");
  const reportId = async response => {
    const r = await response;
    expect(r.status()).toBe(200);
    return (await r.json()).data.report_id;
  };
  const endpoint = "usage/requests", overview = "usage/reports/";
  const wallet = a.locator(".entry-wallet dd");
  const requestChart = a.getByRole("link", { name: "Requests: time chart", exact: true });
  const spend = a.getByRole("article", { name: "Posted charges", exact: true }).locator(".overview-chart-summary");
  const chartData = a.getByRole("table", { name: "Chart data" });

  await wait("usage-before-credit");
  const checkOwnerCorrection = await checkOwnerOverview({ owner, customer: a, origin, api });
  const correctedId = readFileSync(join(exchange, "usage-before-credit"), "utf8");
  const checkOwnerRequestCorrection = await checkOwnerUsage({ owner, customer: a, origin, api, requestId: correctedId });
  const requested = captured();
  await a.goto(origin + "/app/overview");
  const old = await reportId(requested);
  await expect(spend).toHaveText("1.204000 USD");
  await expect(wallet).toHaveText("8.796000 USD");
  await expect(chartData.getByRole("row")).toHaveCount(25);
  const snapshot = await read(overview + old);
  expect(snapshot.from_ms + 86400000).toBe(snapshot.as_of_ms);
  expect(snapshot.totals.spend_micro_usd).toBe("1204000");
  const denied = await foreign.get(api + overview + old);
  expect(denied.status()).toBe(404);
  expect((await denied.json()).error.code).toBe("report_not_found");
  expect((await foreign.get(api + endpoint + "?report=" + old)).status()).toBe(404);
  expect((await foreign.get(api + overview + old + "/csv")).status()).toBe(404);
  const livePath = endpoint + "?id=" + correctedId;
  const privateReceipt = await foreign.get(api + livePath);
  expect(privateReceipt.status()).toBe(200);
  expect((await privateReceipt.json()).data).toEqual({ items: [], diagnostic: null, next_before: null });
  expect((await client.get(api + endpoint)).status()).toBe(400);
  expect((await client.get(api + overview + old + "/csv?before=foreign")).status()).toBe(400);
  expect((await client.get(api + overview + old + "/csv?execution=unknown")).status()).toBe(400);
  expect((await fetch(api + overview + old)).status).toBe(401);
  expect((await fetch(api + overview + old + "/csv")).status).toBe(401);
  expect((await client.post(api + "usage/reports", { data: { hours: 24 } })).status()).toBe(403);

  for (const width of [320, 1120]) {
    await a.setViewportSize({ width, height: 780 });
    const contained = await a.locator(".report-chart-stack").evaluate(el => {
      return document.documentElement.scrollWidth <= innerWidth
        && [...el.querySelectorAll("article")].every(card => card.getBoundingClientRect().right <= innerWidth);
    });
    expect(contained).toBe(true);
    if (process.env.RELAY_TEST_HISTORY_UI_CAPTURE)
      await a.screenshot({ path: join(process.env.RELAY_TEST_HISTORY_UI_CAPTURE, `overview-${width}.png`), fullPage: true });
  }
  await requestChart.focus();
  await requestChart.press("End");
  await expect(a.getByRole("tooltip").filter({ hasText: "View requests" })).toBeVisible();
  await requestChart.press("Enter");
  await expect(a).toHaveURL(new RegExp("/app/usage\\?.*report=" + old + ".*bucket=23"));
  const bucket = 23;
  const path = endpoint + "?report=" + old + "&bucket=" + bucket;
  const original = await read(path);
  expect(original.totals).toEqual(snapshot.buckets[bucket]);
  expect(original.to_ms).toBe(snapshot.as_of_ms);
  expect(original.from_ms + 3600000).toBe(original.to_ms);
  const pending = original.items.find(row => row.request_id === correctedId);
  expect(pending.credit).toBeNull();
  expect(original.diagnostic).toBeUndefined();
  await expect(a.locator(".report-totals").getByText("1.204000 USD")).toBeVisible();
  await a.getByRole("combobox", { name: "Model", exact: true }).selectOption(pending.model);
  await a.getByRole("combobox", { name: "API key", exact: true }).selectOption(pending.key_id);
  await a.getByRole("combobox", { name: "Outcome", exact: true }).selectOption(pending.execution);
  const filter = new URLSearchParams({ bucket: String(bucket), model: pending.model, key_id: pending.key_id, execution: pending.execution });
  const version = overview + old + "/csv?" + filter;
  const pinnedFile = await csv(version);
  const selected = await read(endpoint + "?" + new URLSearchParams({ report: old, ...Object.fromEntries(filter) }));
  const rows = csvRows(pinnedFile);
  expect(rows.map(r => r.request_id)).toEqual(selected.items.map(r => r.request_id));
  expect(rows.every(r => r.execution === pending.execution && r.currency === "USD"
    && r.model === pending.model && r.key_prefix === "rlk_" + pending.key_id.replaceAll("-", "").slice(0, 12))).toBe(true);
  expect(rows.reduce((v, r) => v + BigInt(r.net_charge_micro_usd || "0"), 0n)).toBe(BigInt(selected.totals.spend_micro_usd));
  expect(rows[0].report_from_utc).toBe(new Date(original.from_ms).toISOString());
  expect(rows[0].report_to_utc).toBe(new Date(original.to_ms).toISOString());
  expect(rows.find(r => r.request_id === correctedId).total_tokens).toBe("");
  const selectedUrl = a.url();
  const downloading = a.waitForEvent("download");
  await a.getByRole("button", { name: "Download CSV", exact: true }).click();
  const downloaded = await downloading;
  expect(downloaded.suggestedFilename()).toBe("relay-usage.csv");
  expect(readFileSync(await downloaded.path(), "utf8")).toBe(pinnedFile);

  for (const [width, colorScheme] of [[320, "dark"], [1120, "light"]]) {
    await a.setViewportSize({ width, height: 780 });
    await a.emulateMedia({ colorScheme });
    await a.reload();
    await expect(a.getByRole("combobox", { name: "Outcome", exact: true })).toHaveValue(pending.execution);
    await expect(a.getByRole("button", { name: "Download CSV", exact: true })).toBeVisible();
    expect(await a.locator(".own-history").evaluate(el =>
      document.documentElement.scrollWidth <= innerWidth &&
      [...el.querySelectorAll("select, button")].every(control => control.getBoundingClientRect().right <= innerWidth)
    )).toBe(true);
    if (process.env.RELAY_TEST_HISTORY_UI_CAPTURE)
      await a.screenshot({ path: join(process.env.RELAY_TEST_HISTORY_UI_CAPTURE, `usage-${width}-${colorScheme}.png`), fullPage: true });
  }
  await a.getByRole("link", { name: "Overview", exact: true }).click();
  await expect(spend).toHaveText("1.204000 USD");
  await a.goBack();
  await expect(a).toHaveURL(selectedUrl);
  await expect(a.getByRole("combobox", { name: "API key", exact: true })).toHaveValue(pending.key_id);
  await a.getByRole("button", { name: "Reset filters", exact: true }).click();
  await expect(a.getByRole("combobox", { name: "Model", exact: true })).toBeFocused();
  await expect(a.locator(".report-totals").getByText("1.204000 USD")).toBeVisible();
  const unfilteredUrl = a.url();

  const receipt = a.locator(".own-receipt").filter({ has: a.locator("code", { hasText: correctedId }) });
  await receipt.locator("summary").click();
  await receipt.getByRole("link", { name: "Request details", exact: true }).click();
  await expect(a).toHaveURL(new RegExp("/app/usage\\?hours=24&id=" + correctedId + "$"));
  const direct = await read(livePath);
  expect(direct.items[0].charge_micro_usd).toBe("4000");
  expect(direct.diagnostic).toEqual({ diagnostic_version: 1, coverage: "partial",
    code: "relay_request_incomplete", summary: "interrupted", next_action: "contact_support" });
  const explanation = a.locator(".receipt-diagnostic");
  await expect(explanation.getByText("The response was interrupted.", { exact: true })).toBeVisible();
  await expect(explanation.getByRole("link", { name: "Policies and support", exact: true })).toHaveAttribute("href", "/legal");
  for (const [width, theme] of [[320, "dark"], [1120, "light"]]) {
    await a.setViewportSize({ width, height: 780 });
    await a.evaluate(theme => localStorage.setItem("relay-hosted-theme", theme), theme);
    await a.reload();
    await expect(a.locator(".hosted-entry")).toHaveAttribute("data-theme", theme);
    await expect(explanation).toBeVisible();
    expect(await explanation.evaluate(el =>
      document.documentElement.scrollWidth <= innerWidth && [...el.querySelectorAll("a,button,code")].every(node =>
        node.getBoundingClientRect().right <= innerWidth))).toBe(true);
    if (process.env.RELAY_TEST_HISTORY_UI_CAPTURE)
      await a.screenshot({ path: join(process.env.RELAY_TEST_HISTORY_UI_CAPTURE, `customer-diagnostic-${width}-${theme}.png`), fullPage: true });
  }
  await a.evaluate(() => Object.defineProperty(navigator, "clipboard", {
    configurable: true, value: { writeText: async text => { window.fixtureSupportSummary = text; } },
  }));
  await explanation.getByRole("button", { name: "Copy support summary", exact: true }).click();
  await expect.poll(() => a.evaluate(() => JSON.parse(window.fixtureSupportSummary ?? "null"))).toEqual({
    api_version: 1, diagnostic_version: 1, started_at: new Date(direct.items[0].started_at_ms).toISOString(),
    request_id: correctedId, code: direct.diagnostic.code, coverage: direct.diagnostic.coverage,
    execution: direct.items[0].execution, accounting: direct.items[0].accounting,
  });
  await a.goto(unfilteredUrl);
  await expect(a.locator(".report-totals").getByText("1.204000 USD")).toBeVisible();
  issue("usage-pinned");
  await wait("usage-credit-posted");
  await checkOwnerCorrection();
  await checkOwnerRequestCorrection();
  expect(await read(path)).toEqual(original);
  expect(await csv(version)).toBe(pinnedFile);
  await a.reload();
  await expect(a.locator(".report-totals").getByText("1.204000 USD")).toBeVisible();
  await a.getByRole("link", { name: "Overview", exact: true }).click();
  await expect(spend).toHaveText("1.204000 USD");
  await expect(wallet).toHaveText("8.799400 USD");
  const refresh = captured();
  await a.getByRole("button", { name: "Refresh usage", exact: true }).click();
  const current = await reportId(refresh);
  expect(current).not.toBe(old);
  await expect(spend).toHaveText("1.200600 USD");
  const recent = await read(endpoint + "?report=" + current);
  const corrected = recent.items.find(v => v.request_id === correctedId);
  expect(corrected.charge_micro_usd).toBe("4000");
  expect(corrected.total_tokens).toBe("20");
  expect(corrected.usage_quality).toBe("final");
  expect(corrected.credit.amount_micro_usd).toBe("3400");
  const live = await read(livePath);
  expect(live.diagnostic).toEqual(direct.diagnostic);
  expect(live.items[0].credit.amount_micro_usd).toBe("3400");
  const refreshed = csvRows(await csv(overview + current + "/csv?key_id=" + pending.key_id));
  const credit = refreshed.find(r => r.request_id === correctedId);
  expect(credit.net_charge_micro_usd).toBe("600");
  expect(credit.posted_credit_micro_usd).toBe("3400");
  expect(credit.total_tokens).toBe("20");
  issue("usage-checked");
  return { current, corrected };
}
