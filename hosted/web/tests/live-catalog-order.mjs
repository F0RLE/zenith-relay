import { expect } from "@playwright/test";
import { join } from "node:path";
import { navigateCustomer } from "./customer-shell.mjs";

export async function checkLiveCatalogOrder({ owner, customer, endpoint, list, csrf }) {
  const url = new URL("model-display-order", endpoint).href;
  const read = async () => (await (await owner.request.get(url)).json()).data;
  const rules = owner.getByRole("region", { name: "Model rules", exact: true });
  const grip = rules.getByRole("button", { name: "Move company OpenAI", exact: true });
  const ids = model => model.map(item => item.model);
  const order = async () => ids((await read()).items);
  const published = await list();
  const first = await read();
  expect(first.items).toEqual([{ model: "fixture-text", publisher: null }, { model: "review-text", publisher: "openai" }]);
  const command = { expected_revision: first.revision, model_display_order: ["review-text", "fixture-text"] };
  const put = async (data, opts = {}) => (opts.client ?? owner.request).put(url, {
    headers: { Origin: opts.origin ?? new URL(url).origin, "X-Relay-CSRF": opts.token ?? await csrf() }, data,
  });
  expect((await customer.request.get(url)).status()).toBe(401);
  expect((await put(command, { client: customer.request })).status()).toBe(401);
  expect((await put(command, { origin: "https://outside.invalid" })).status()).toBe(403);
  expect((await put(command, { token: "0".repeat(64) })).status()).toBe(403);
  const expand = rules.getByRole("button", { name: "Collapse OpenAI models", exact: true });
  await expand.click();
  await expect(rules.locator('[data-model-id="review-text"]')).toBeHidden();
  expect(await order()).toEqual(["fixture-text", "review-text"]);
  await rules.getByRole("button", { name: "Expand OpenAI models", exact: true }).click();

  // Another view changes the relative order after this view has read it.
  const concurrent = await put(command);
  expect(concurrent.status()).toBe(200);
  const conflict = owner.waitForResponse(r => r.url() === url && r.request().method() === "PUT" && r.status() === 409);
  await grip.press("ArrowUp");
  expect((await (await conflict).json()).error.code).toBe("model_order_changed");
  await expect.poll(order).toEqual(["review-text", "fixture-text"]);
  await expect(rules.locator(".live-rule-group").first()).toHaveAttribute("data-group-id", "catalog-openai");
  await expect(rules.getByRole("alert")).toHaveCount(0);

  const response = owner.waitForResponse(r => r.url() === url && r.request().method() === "PUT" && r.ok());
  await grip.press("ArrowDown");
  await response;
  await expect(grip).toBeFocused();
  await expect.poll(order).toEqual(["fixture-text", "review-text"]);
  const poolUrl = owner.url();
  await owner.reload();
  await owner.getByRole("button", { name: "Model rules", exact: true }).click();
  await expect(rules.locator(".live-rule-group").first()).toHaveAttribute("data-group-id", "catalog-other");
  await grip.press("Home");
  await expect.poll(order).toEqual(["review-text", "fixture-text"]);
  await expect(rules.locator(".live-rule-group").first()).toHaveAttribute("data-group-id", "catalog-openai");
  const after = await list();
  expect(after).toEqual([published[1], published[0]]);
  const catalog = customer.locator(".catalog-table");
  await navigateCustomer(customer, "Models");
  await customer.reload();
  await expect(catalog.locator(".catalog-company")).toHaveText(["OpenAI", "Other"]);
  await expect(catalog.locator(".catalog-model-name")).toHaveText(ids(after));
  await expect(catalog.locator(".catalog-editor,.owner-model-drag-handle")).toHaveCount(0);
  await navigateCustomer(customer, "Status");
  const statusRows = customer.locator(".model-status-table .model-status-name");
  await expect(statusRows).toHaveText(ids(after));
  await customer.getByRole("button", { name: "7 days", exact: true }).click();
  await expect(statusRows).toHaveText(ids(after));
  if (process.env.RELAY_TEST_CATALOG_CAPTURE) {
    const viewport = owner.viewportSize();
    const preference = await owner.evaluate(() => localStorage.getItem("relay-hosted-theme"));
    for (const [width, theme] of [[320, "dark"], [1024, "light"]]) {
      await owner.setViewportSize({ width, height: 900 });
      await owner.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
      await owner.evaluate(theme => localStorage.setItem("relay-hosted-theme", theme), theme);
      await owner.goto(poolUrl);
      await expect(owner.locator(".hosted-entry")).toHaveAttribute("data-theme", theme);
      await owner.getByRole("button", { name: "Model rules", exact: true }).click();
      await expect(rules.locator(".live-rule-group").first()).toHaveAttribute("data-group-id", "catalog-openai");
      expect(await rules.evaluate(el => el.scrollWidth <= el.clientWidth && document.documentElement.scrollWidth <= innerWidth &&
        [...el.querySelectorAll("button,input,select")].every(control => {
          const { left, right } = control.getBoundingClientRect();
          return left >= 0 && right <= innerWidth;
        }))).toBe(true);
      await owner.screenshot({ path: join(process.env.RELAY_TEST_CATALOG_CAPTURE, `live-model-rules-${width}-${theme}.png`) });
    }
    await owner.evaluate(value => {
      if (value === null) localStorage.removeItem("relay-hosted-theme");
      else localStorage.setItem("relay-hosted-theme", value);
    }, preference);
    await owner.goto(poolUrl);
    await owner.getByRole("button", { name: "Model rules", exact: true }).click();
    if (viewport) await owner.setViewportSize(viewport);
  }
}
