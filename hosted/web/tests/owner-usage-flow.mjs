import { expect } from "@playwright/test";
import { join } from "node:path";
import { fixtureOrigin } from "../../test-support/files.mjs";

export async function checkOwnerUsage({ owner, customer, origin, api, requestId }) {
  origin = fixtureOrigin(origin);
  if (api !== origin + "/api/hosted/v1/" || !/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(requestId))
    throw new Error("Invalid owner usage fixture target");
  const base = api + "admin/usage/", detail = base + "requests/" + requestId;
  const customerHttp = customer.context().request;
  const context = await owner.context().browser().newContext({ storageState: await owner.context().storageState() });
  const page = await context.newPage();
  const http = page.context().request;
  const read = async url => {
    const r = await http.get(url);
    expect(r.status()).toBe(200);
    expect(r.headers()["cache-control"]).toBe("no-store");
    return (await r.json()).data;
  };
  for (const url of [base + "requests", base + "errors", detail]) {
    expect((await customerHttp.get(url)).status()).toBe(401);
    const anonymousContext = await owner.context().browser().newContext();
    try { expect((await anonymousContext.request.get(url, { maxRedirects: 0 })).status()).toBe(401); }
    finally { await anonymousContext.close(); }
  }
  const anonymous = await customerHttp.get(origin + "/admin/usage/requests/" + requestId, { maxRedirects: 0 });
  expect(anonymous.status()).toBe(303);
  expect(anonymous.headers().location).toBe("/admin/login");
  expect((await http.post(detail, { data: {} })).status()).toBe(405);
  for (const query of ["hours=0", "from_ms=1", "hours=24&hours=168", "accounting=closed", "tab=errors", "model=space%20name",
    "stage=unknown&stage=dispatch", "origin=other", "severity=severe", "category=preview", "code=unbounded%20text"])
    expect((await http.get(base + "requests?" + query)).status()).toBe(400);
  const defaultList = await read(base + "requests");
  expect(defaultList.to_ms - defaultList.from_ms).toBe(168 * 3600000);
  const exact = await read(detail);
  expect(exact.receipt.request_id).toBe(requestId);
  expect(exact.receipt.charge_micro_usd).toBe("4000");
  expect(exact.receipt.credit).toBeNull();
  expect(exact.attempt.source_id).toMatch(/^[0-9a-f-]{36}$/);
  expect(exact.timeline.map(x => x.event)).toContain("dispatch_permitted");
  expect(exact.timeline.map(x => x.event)).toContain("hold_closed");
  expect(exact.diagnostic_coverage).toBe("partial");
  expect(exact.diagnostics).toHaveLength(1);
  expect(exact.diagnostics[0]).toMatchObject({ request_id: requestId, attempt_id: exact.attempt.id,
    stage: "upstream_read", code: "upstream_transport_body", origin: null, category: "transport", severity: "error" });
  const wire = JSON.stringify(exact);
  for (const forbidden of ["input_tag", "send_tag", "base_url", "proxy", "authorization", "prompt", "api_secret", "response_body"])
    expect(wire).not.toContain(forbidden);

  const overview = page.waitForResponse(r => r.url() === api + "admin/overview/usage?hours=24");
  await page.goto(origin + "/admin/overview");
  const chart = (await (await overview).json()).data;
  const plot = page.getByRole("link", { name: "Requests: time chart", exact: true });
  await plot.focus();
  await plot.press("End");
  await expect(page.getByRole("tooltip").filter({ hasText: "View requests" })).toBeVisible();
  await plot.press("Enter");
  await expect(page).toHaveURL(new RegExp(`/admin/usage\\?from_ms=${chart.as_of_ms - 3600000}&to_ms=${chart.as_of_ms}$`));
  const listUrl = page.url();
  const requestLink = page.getByRole("link").filter({ hasText: requestId });
  await expect(requestLink).toHaveCount(1);
  await requestLink.focus();
  await requestLink.press("Enter");
  const net = page.locator(".owner-case-net");
  await expect(net).toHaveText("0.004000 USD");
  await expect(page.getByText("Final usage not confirmed", { exact: true })).toBeVisible();
  const diagnostics = page.locator(".owner-diagnostics");
  await expect(diagnostics.getByRole("heading", { name: "Diagnostics", exact: true })).toBeVisible();
  await expect(diagnostics.getByText("Reading the upstream response was interrupted", { exact: true })).toBeVisible();
  await expect(diagnostics.getByText("upstream_transport_body", { exact: true })).toBeVisible();
  await page.getByRole("link", { name: "Back to usage", exact: true }).click();
  await expect(page).toHaveURL(listUrl);
  await page.getByRole("link", { name: "Errors", exact: true }).click();
  await expect(page.getByRole("link", { name: "Errors", exact: true })).toHaveAttribute("aria-current", "page");
  const errorsUrl = new URL(page.url());
  errorsUrl.searchParams.delete("tab");
  const errors = await read(base + "errors" + errorsUrl.search);
  const cases = errors.items.filter(x => x.request_id === requestId);
  expect(cases).toHaveLength(1);
  expect(cases[0].diagnostic).toEqual(exact.diagnostics[0]);
  expect(errors.items.every(x => x.issues.length > 0)).toBe(true);
  expect(errors.items.length).toBeLessThan(defaultList.items.length);
  await expect(requestLink).toContainText("Reading the upstream response · Reading the upstream response was interrupted");
  await page.getByText("Additional filters", { exact: true }).click();
  await page.getByRole("textbox", { name: "Model", exact: true }).fill(exact.receipt.model);
  await page.getByRole("combobox", { name: "Stage", exact: true }).selectOption("upstream_read");
  await page.getByRole("combobox", { name: "Category", exact: true }).selectOption("transport");
  await page.getByRole("combobox", { name: "Severity", exact: true }).selectOption("error");
  await page.getByRole("textbox", { name: "Error code", exact: true }).fill("upstream_transport_body");
  await page.getByRole("button", { name: "Apply filters", exact: true }).click();
  await expect(page).toHaveURL(/model=fixture-text.*stage=upstream_read.*code=upstream_transport_body.*tab=errors/);
  await expect(page.locator(".owner-request-list").getByRole("listitem")).toHaveCount(1);
  await expect(requestLink).toBeVisible();
  const filterSummary = page.locator(".owner-usage-filters > summary");
  await expect(filterSummary).toContainText("(5)");
  await expect(page.getByRole("combobox", { name: "Stage", exact: true })).toBeHidden();
  const filteredUrl = page.url();
  const lookup = page.getByRole("searchbox", { name: "Request ID", exact: true });
  await lookup.fill("not-an-id");
  await lookup.press("Enter");
  await expect(page.getByRole("alert")).toHaveText("Enter the full request ID.");
  await lookup.fill("11111111-1111-4111-8111-111111111111");
  await lookup.press("Enter");
  await expect(page.getByText("Request not found in retained history.")).toBeVisible();
  await page.getByRole("link", { name: "Back to usage", exact: true }).click();
  await lookup.fill(requestId.toUpperCase());
  await lookup.press("Enter");
  await expect(net).toHaveText("0.004000 USD");

  const detailUrl = page.url();
  const checkContained = () => page.locator(".owner-usage").evaluate(el =>
    document.documentElement.scrollWidth <= innerWidth && [...el.querySelectorAll("button,input,select,a,code,time")].every(node => {
      const box = node.getBoundingClientRect();
      return box.width === 0 || (box.left >= 0 && box.right <= innerWidth);
    }));
  for (const [width, theme, locale, label, listLabel] of [
    [320, "dark", "ru", "Детали запроса", "Использование"], [320, "light", "zh-CN", "请求详情", "用量"],
    [1120, "light", "en", "Request details", "Usage"],
  ]) {
    await page.setViewportSize({ width, height: 810 });
    await page.emulateMedia({ colorScheme: theme === "light" ? "dark" : "light" });
    await page.evaluate(([theme, locale]) => {
      localStorage.setItem("relay-hosted-theme", theme);
      localStorage.setItem("relay-hosted-locale", locale);
    }, [theme, locale]);
    await page.reload();
    await expect(page.getByRole("heading", { name: label, exact: true })).toBeVisible();
    await expect(net).toBeVisible();
    await expect(diagnostics.getByText("upstream_transport_body", { exact: true })).toBeVisible();
    await expect(page.locator(".hosted-entry")).toHaveAttribute("data-theme", theme);
    const menu = page.locator(".hosted-tools .mobile-menu");
    if (width === 320) await menu.click();
    await expect(page.locator(".relay-sidebar").getByRole("link", { name: listLabel, exact: true })).toHaveAttribute("aria-current", "location");
    if (width === 320) await page.keyboard.press("Escape");
    expect(await net.evaluate(el => el.getBoundingClientRect().height <= parseFloat(getComputedStyle(el).lineHeight) + 0.5)).toBe(true);
    expect(await checkContained()).toBe(true);
    if (process.env.RELAY_TEST_HISTORY_UI_CAPTURE)
      await page.screenshot({ path: join(process.env.RELAY_TEST_HISTORY_UI_CAPTURE, `owner-request-${width}-${locale}-${theme}.png`), fullPage: true });
    await page.goto(filteredUrl);
    await expect(page.getByRole("heading", { name: listLabel, exact: true })).toBeVisible();
    await expect(requestLink).toBeVisible();
    expect(await page.locator(".owner-request-lookup").evaluate(el => {
      const field = el.querySelector("input").getBoundingClientRect(), button = el.querySelector("button").getBoundingClientRect();
      return field.height >= 44 && Math.abs(field.height - button.height) < 1;
    })).toBe(true);
    expect(await page.locator(".owner-usage-controls").evaluate(el => {
      const buttons = [...el.querySelectorAll("button")].map(button => button.getBoundingClientRect());
      return buttons.every(box => box.height >= 44 && Math.abs(box.top - buttons[0].top) < 1);
    })).toBe(true);
    expect(await checkContained()).toBe(true);
    if (process.env.RELAY_TEST_HISTORY_UI_CAPTURE)
      await page.screenshot({ path: join(process.env.RELAY_TEST_HISTORY_UI_CAPTURE, `owner-errors-${width}-${locale}-${theme}.png`), fullPage: true });
    await filterSummary.focus();
    await filterSummary.press("Enter");
    await expect(page.locator('.owner-usage-filters select[name="stage"]')).toHaveValue("upstream_read");
    expect(await checkContained()).toBe(true);
    await page.goto(detailUrl);
  }
  return async () => {
    try {
      await page.getByRole("button", { name: "Refresh request", exact: true }).click();
      await expect(net).toHaveText("0.000600 USD");
      await expect(page.getByText("Charge correction posted", { exact: true })).toBeVisible();
      const current = await read(detail);
      expect(current.receipt.credit.amount_micro_usd).toBe("3400");
      expect(current.receipt.total_tokens).toBe("20");
      expect(current.receipt.usage_quality).toBe("final");
      expect(current.diagnostics).toEqual(exact.diagnostics);
      expect(current.timeline.map(e => e.event)).toContain("late_usage");
      expect(current.timeline.map(e => e.event)).toContain("credit_posted");
      expect((await read(base + "requests" + new URL(listUrl).search)).items.find(x => x.request_id === requestId).posted_micro_usd).toBe("600");
    } finally { await context.close(); }
  };
}
