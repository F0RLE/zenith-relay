import { expect } from "@playwright/test";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

export async function checkSourceCopy(page, origin, exchange) {
  const wait = name => expect.poll(() => existsSync(join(exchange, name)), { timeout: 25000 }).toBe(true);
  const notify = name => writeFileSync(join(exchange, name), "", { flag: "wx" });
  const api = "/api/hosted/v1";
  await expect.poll(() => page.evaluate(async path => {
    try { return (await fetch(path)).status; } catch { return 0; }
  }, api + "/public/config"), { timeout: 15000 }).toBe(200);
  await page.goto(origin + "/admin/connections");
  await page.getByRole("button", { name: "Sign in with a passkey" }).click();
  await page.getByRole("link", { name: "Connections", exact: true }).click();
  await page.getByText("Desktop source copy", { exact: true }).click();
  notify("owner-ready");
  await wait("review");
  const code = readFileSync(join(exchange, "review"), "utf8");
  const input = page.getByLabel("Review code from Desktop");
  await input.fill(code);
  const decision = status => page.waitForResponse(r =>
    r.url() === origin + api + "/admin/source-copy" && r.request().method() === "POST" && r.status() === status);
  const preview = decision(200);
  await page.getByRole("button", { name: "Review source" }).click();
  expect((await (await preview).json()).data.state).toBe("awaiting_approval");
  await expect(page.getByText("Synthetic copied source", { exact: true })).toBeVisible();
  await expect(page.getByText("Device: Copy fixture")).toBeVisible();
  await expect(page.getByText(/Authentication included/)).toBeVisible();

  const stale = decision(403), accepted = decision(200);
  await page.getByRole("button", { name: "Approve one copy" }).click();
  expect((await (await stale).json()).error.code).toBe("fresh_passkey_required");
  expect((await accepted).request().postDataJSON()).toEqual({ code, approve: true });
  await expect(page.getByRole("status").filter({ hasText: "Approved for 10 minutes" })).toBeVisible();
  notify("approved");
  await wait("committed");
  await page.getByRole("button", { name: "Check result" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Saved outside the pool." })).toBeVisible();
  const list = page.getByRole("region", { name: "API providers" });
  await expect(list.getByText("Synthetic copied source", { exact: true })).toBeVisible();
  await expect(list.getByText("Proxy assigned", { exact: true })).toBeVisible();
  await page.setViewportSize({ width: 320, height: 760 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await expect(list.getByText("Synthetic copied source", { exact: true })).toBeVisible();
}
