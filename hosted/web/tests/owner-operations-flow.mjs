import { expect } from "@playwright/test";
import { join } from "node:path";

export async function checkOwnerOperations({ owner, a, b, origin, api, issue, wait }) {
  const status = api + "admin/operations/status", events = api + "admin/operations/events";
  const http = owner.context().request;
  for (const endpoint of [status, events]) {
    expect((await fetch(endpoint)).status).toBe(401);
    for (const customer of [a, b]) expect((await customer.context().request.get(endpoint)).status()).toBe(401);
  }
  for (const query of ["?before=0", "?before=01", "?before=1&before=2", "?log=any"])
    expect((await http.get(events + query)).status()).toBe(400);

  await wait("operations-failed");
  await owner.setViewportSize({ width: 1120, height: 760 });
  await owner.goto(origin + "/admin/overview");
  await owner.getByRole("link", { name: "Operations", exact: true }).click();
  await expect(owner).toHaveURL(origin + "/admin/operations");
  const page = owner.locator(".owner-operations");
  const jobs = owner.getByRole("region", { name: "Background jobs" });
  const history = owner.getByRole("region", { name: "Recent events" });
  const reconciliation = jobs.getByRole("listitem").filter({ hasText: "Request reconciliation" });
  const refresh = page.getByRole("button", { name: "Refresh operations" });
  await expect(reconciliation.getByText("Failed", { exact: true })).toBeVisible();
  await expect(history.getByText("Failed", { exact: true })).toBeVisible();
  await expect(jobs.getByRole("listitem")).toHaveCount(6);
  const response = await http.get(events);
  expect(response.status()).toBe(200);
  expect(response.headers()["cache-control"]).toBe("no-store");
  expect((await response.json()).data.items).toEqual([
    { id: expect.stringMatching(/^\d+$/), job: "request_reconciliation", code: "job_failed",
      first_at_ms: expect.any(Number), last_at_ms: expect.any(Number), observations: 1 },
  ]);
  issue("operations-failure-seen");
  await wait("operations-recovered");
  await refresh.click();
  await expect(reconciliation.getByText("Succeeded", { exact: true })).toBeVisible();
  await expect(history.getByText("Recovered", { exact: true })).toBeVisible();
  await expect(history.getByText("Failed", { exact: true })).toBeVisible();

  for (const [width, theme, locale, title] of [
    [320, "dark", "ru", "Операции"], [320, "light", "zh-CN", "运维"], [1120, "light", "en", "Operations"],
  ]) {
    await owner.setViewportSize({ width, height: 760 });
    await owner.evaluate(([theme, locale]) => {
      localStorage.setItem("relay-hosted-theme", theme);
      localStorage.setItem("relay-hosted-locale", locale);
    }, [theme, locale]);
    await owner.reload();
    await expect(owner.getByRole("heading", { name: title, exact: true })).toBeVisible();
    await expect(owner.locator(".hosted-entry")).toHaveAttribute("data-theme", theme);
    await expect(page.locator(".operation-list")).toHaveCount(2);
    expect(await page.evaluate(el => document.documentElement.scrollWidth <= innerWidth &&
      [...el.querySelectorAll("li, time, button")].every(node => {
        const rect = node.getBoundingClientRect();
        return rect.left >= 0 && rect.right <= innerWidth;
      }))).toBe(true);
    if (process.env.RELAY_TEST_HISTORY_UI_CAPTURE)
      await owner.screenshot({ path: join(process.env.RELAY_TEST_HISTORY_UI_CAPTURE, `owner-operations-${width}-${locale}-${theme}.png`), fullPage: true });
  }
  await owner.route(status, route => route.abort(), { times: 1 });
  await refresh.focus();
  await refresh.press("Enter");
  await expect(jobs.getByRole("alert")).toBeVisible();
  await expect(reconciliation).toHaveCount(0);
  await refresh.click();
  await expect(reconciliation.getByText("Succeeded", { exact: true })).toBeVisible();
  issue("operations-checked");
}
