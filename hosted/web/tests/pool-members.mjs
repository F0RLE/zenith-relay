import { expect } from "@playwright/test";
import { join } from "node:path";

export async function addPoolModel(owner, source, model) {
  await owner.getByRole("button", { name: "Add member", exact: true }).click();
  const picker = owner.getByRole("dialog", { name: "Add member", exact: true });
  const add = picker.getByRole("button", { name: "Add to pool", exact: true });
  await expect(add).toBeDisabled();
  await picker.getByRole("radio", { name: `${source} ${model}`, exact: true }).check();
  await add.click();
  await expect(picker).toBeHidden();
  await expect(owner.getByRole("button", { name: "Add member", exact: true })).toBeEnabled();
  const member = owner.getByRole("article", { name: source, exact: true });
  await expect(member.getByText("In pool", { exact: true })).toBeVisible();
}

export async function checkPoolActivity({ owner, customer, api, sourceId, model }) {
  const endpoint = api + "admin/pools/activity";
  expect((await fetch(endpoint)).status).toBe(401);
  expect((await customer.request.get(endpoint)).status()).toBe(401);
  expect((await owner.request.get(endpoint + "?source_id=" + sourceId)).status()).toBe(400);
  await owner.bringToFront();
  await owner.getByRole("button", { name: "Members", exact: true }).click();
  const response = await owner.request.get(endpoint);
  expect(response.headers()["cache-control"]).toBe("no-store");
  expect((await response.json()).data).toEqual({ strategy: "automatic", member_order: [], items: [
    { source_id: sourceId, source: "Test connection", model, requests: 1 },
  ] });
  const card = owner.getByRole("article", { name: "Test connection", exact: true });
  const running = owner.getByText(`In progress (1): ${model}`, { exact: true });
  await expect(card).toHaveAttribute("data-current", "true");
  await expect(running).toBeVisible();
  await expect(owner.getByText("Automatic", { exact: true })).toBeVisible();
  await expect(owner.getByRole("button", { name: "Pool rotation settings", exact: true })).toBeEnabled();
  await expect(owner.getByRole("button", { name: /request speed/i })).toHaveCount(0);
  await expect(owner.locator(".owner-pool-runtime")).not.toContainText(/next/i);
  const offline = route => route.fulfill({ status: 503 });
  await owner.route(endpoint, offline);
  try {
    await expect(owner.getByText("Activity not received yet", { exact: true })).toBeVisible();
    await expect(card).not.toHaveAttribute("data-current", "true");
    await expect(running).toHaveCount(0);
  } finally {
    await owner.unroute(endpoint, offline);
  }
  await expect(card).toHaveAttribute("data-current", "true");
  const previous = owner.viewportSize();
  if (process.env.RELAY_TEST_CATALOG_CAPTURE) {
    const prefs = await owner.evaluate(() => ["relay-hosted-theme", "relay-hosted-locale"].map(key => [key, localStorage.getItem(key)]));
    for (const [width, theme, locale] of [[320, "dark", "en"], [1024, "light", "ru"]]) {
      await owner.setViewportSize({ width, height: 760 });
      await owner.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
      await owner.evaluate(([theme, locale]) => {
        localStorage.setItem("relay-hosted-theme", theme);
        localStorage.setItem("relay-hosted-locale", locale);
      }, [theme, locale]);
      await owner.reload();
      await expect(owner.locator(".hosted-entry")).toHaveAttribute("data-theme", theme);
      await expect(owner.locator(".hosted-entry")).toHaveAttribute("lang", locale);
      await expect(card).toHaveAttribute("data-current", "true");
      expect(await owner.locator(".owner-pool-active-models > span").evaluate(el =>
        el.scrollWidth <= el.clientWidth && document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await owner.screenshot({
        path: join(process.env.RELAY_TEST_CATALOG_CAPTURE, `pool-active-${width}-${theme}.png`), fullPage: true,
      });
    }
    await owner.evaluate(prefs => {
      for (const [key, value] of prefs) {
        if (value === null) localStorage.removeItem(key); else localStorage.setItem(key, value);
      }
    }, prefs);
    await owner.reload();
  }
  if (previous) await owner.setViewportSize(previous);
  await owner.emulateMedia({ colorScheme: null, reducedMotion: null });
  await owner.getByRole("button", { name: "Model rules", exact: true }).click();
}
