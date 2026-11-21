import { expect } from "@playwright/test";
import { join } from "node:path";
import { addPoolModel } from "./pool-members.mjs";
import { checkLiveCatalogOrder } from "./live-catalog-order.mjs";
import { checkMemberModels } from "./member-models-flow.mjs";
import { checkPoolRotation } from "./pool-rotation-flow.mjs";

const model = "review-text";
const decimal = value => {
  const [whole, fraction = ""] = value.split(".");
  return `${whole}.${fraction.padEnd(12, "0")}`;
};
const prices = (input, output, cached) => ({ input: decimal(input), output: decimal(output), cached: decimal(cached) });
const aCost = prices("2", "3", "0.5"), bCost = prices("1", "6", "0.25");
const inherited = prices("2.5", "6.6", "0.625"), custom = prices("3", "9", "0.75"), fixed = prices("3", "5", "0.4");
const apiDefault = { mode: "api_default" };

export async function checkSellingReview({ owner, customer, rules, list, send, csrf, endpoint, proof, issue, wait }) {
  const admit = name => addPoolModel(owner, name, model);
  const rulesTab = owner.getByRole("button", { name: "Model rules" });
  const editor = owner.locator(".catalog-editor").filter({ has: owner.getByText(model, { exact: true }) });
  const buy = editor.getByRole("group", { name: "Purchase prices" });
  const sell = editor.getByRole("group", { name: "Selling prices" });
  const mode = sell.getByLabel("Selling price rule");
  const source = editor.getByRole("combobox", { name: "Source", exact: true });
  const review = editor.getByRole("button", { name: "Review prices" });
  const save = editor.getByRole("button", { name: "Save prices" });
  const publish = editor.getByRole("button", { name: "Publish to testers" });
  const markup = buy.getByLabel("API default markup (%)");
  const row = async (name = "Test connection", id = model) => (await rules()).find(r => r.model === id && r.source === name);
  const customerRate = async () => (await list()).find(r => r.model === model);
  const update = async (r, action, fields = {}, status = 200, error) => {
    const response = await send(owner.context().request, r.id, await csrf(), {
      expected_revision: r.revision, operation_id: crypto.randomUUID(), action, ...fields,
    });
    const data = await response.json();
    expect(response.status(), data.error?.code).toBe(status);
    if (error) expect(data.error.code).toBe(error);
  };
  const fill = async (group, rate) => {
    for (const [name, key] of [["Input", "input"], ["Output", "output"], ["Cache read", "cached"]])
      await group.getByLabel(name, { exact: true }).fill(rate[key]);
  };
  const saved = async () => { await save.click(); await expect(save).toBeHidden(); };
  const draft = expected => expect.poll(async () => (await row()).draft).toEqual(expected);
  const stale = expected => expect.poll(async () => (await row()).sale_rule.stale).toBe(expected);
  const poolUrl = owner.url();
  const openConnection = async (name, admitted = true) => {
    await owner.goto(new URL("/admin/connections", poolUrl).href);
    const card = owner.getByRole("article", { name, exact: true });
    expect(await card.getAttribute("data-source-id")).toBe((await row(name)).source_id);
    const trigger = card.getByRole("button", { name: /Verified models/ });
    await trigger.press("Enter");
    const dialog = owner.getByRole("dialog", { name: admitted ? "Models" : "Verified models", exact: true });
    await expect(dialog).toBeVisible();
    return { dialog, trigger };
  };
  const backToPool = async () => {
    await owner.goto(poolUrl);
    await rulesTab.click();
  };

  await admit("Test connection");
  const outside = await openConnection("Second connection", false);
  await expect(outside.dialog.getByText(model, { exact: true })).toBeVisible();
  await expect(outside.dialog.getByRole("group", { name: "Purchase prices" })).toHaveCount(0);
  await owner.keyboard.press("Escape");
  await expect(outside.trigger).toBeFocused();

  const connection = await openConnection("Test connection");
  const reviewEditor = connection.dialog.locator(".catalog-editor").filter({ has: owner.getByText(model, { exact: true }) });
  const connectionBuy = reviewEditor.getByRole("group", { name: "Purchase prices" });
  await expect(connection.dialog.locator(".catalog-editor")).toHaveCount(2);
  await expect(connection.dialog.getByLabel("API default markup (%)")).toHaveCount(1);
  await expect(reviewEditor.getByRole("combobox", { name: "Source", exact: true })).toHaveCount(0);
  await fill(connectionBuy, aCost);
  const connectionSave = reviewEditor.getByRole("button", { name: "Save prices", exact: true });
  await connectionSave.click();
  await expect(connectionSave).toBeHidden();
  expect((await row("Second connection")).purchase).toBeNull();
  await backToPool();
  await expect(buy.getByLabel("Input", { exact: true })).toHaveValue("2");
  await mode.selectOption("api_default");
  await review.click();
  await expect(owner.getByRole("alert")).toHaveText(/No admitted source/);
  expect((await row()).draft).toBeNull();
  expect((await row()).source_markup).toEqual({ revision: "0", percent: null });

  await openConnection("Test connection");
  await connection.dialog.getByLabel("API default markup (%)").fill("0");
  const saveDefault = connection.dialog.getByRole("button", { name: "Save", exact: true });
  await saveDefault.click();
  await expect(saveDefault).toBeHidden();
  expect((await row()).source_markup).toEqual({ revision: "1", percent: decimal("0") });
  expect((await row("Test connection", "fixture-text")).source_markup).toEqual((await row()).source_markup);
  for (const [width, theme] of [[1024, "light"], [320, "dark"]]) {
    await owner.setViewportSize({ width, height: 800 });
    await owner.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
    await openConnection("Test connection");
    await expect(owner.locator(".hosted-entry")).toHaveAttribute("data-theme", theme);
    await expect(connection.dialog.getByLabel("API default markup (%)")).toHaveValue("0");
    expect(await connection.dialog.evaluate(el => {
      const { left, right, top, bottom } = el.getBoundingClientRect();
      return left >= 0 && right <= innerWidth && top >= 0 && bottom <= innerHeight && el.scrollWidth <= el.clientWidth;
    })).toBe(true);
    await reviewEditor.getByRole("combobox", { name: "Selling price rule" }).scrollIntoViewIfNeeded();
    if (process.env.RELAY_TEST_CATALOG_CAPTURE)
      await owner.screenshot({ path: join(process.env.RELAY_TEST_CATALOG_CAPTURE, `connection-prices-${width}-${theme}.png`) });
  }
  await owner.setViewportSize({ width: 1024, height: 800 });
  // No form is mounted under a temporary publisher while the layout is in flight.
  const layoutUrl = new URL("model-display-order", endpoint).href;
  let releaseLayout, layoutRequested;
  const layoutGate = new Promise(resolve => { releaseLayout = resolve; });
  const layoutSeen = new Promise(resolve => { layoutRequested = resolve; });
  const delayLayout = async route => {
    layoutRequested();
    await layoutGate;
    await route.continue();
  };
  await owner.route(layoutUrl, delayLayout);
  try {
    await backToPool();
    await layoutSeen;
    const panel = owner.getByRole("region", { name: "Model rules", exact: true });
    await expect(panel.getByRole("status")).toHaveText("Connecting…");
    await expect(panel.locator(".catalog-editor")).toHaveCount(0);
    releaseLayout();
    await expect(panel.locator('[data-group-id="catalog-openai"] [data-model-id="review-text"]')).toBeVisible();
  } finally {
    releaseLayout();
    await owner.unroute(layoutUrl, delayLayout);
  }
  await expect(markup).toHaveValue("0");
  await mode.selectOption("api_default");
  await review.click();
  await draft(aCost);
  await expect(review).toBeHidden();
  expect(await customerRate()).toBeUndefined();
  await update(await row(), "review", { sale_rule: apiDefault, rates: fixed }, 400, "invalid_model_rule");
  await update(await row(), "review", { sale_rule: { mode: "model_markup", percent: "0.000000000001" } },
    409, "unrepresentable_markup");
  await markup.fill("25"); await saved();
  await expect(editor.getByText(/Purchase inputs changed/)).toBeVisible();
  await expect(publish).toBeDisabled();
  await update(await row(), "publish", {}, 409, "price_review_stale");
  await review.click();
  await draft(prices("2.5", "3.75", "0.625"));
  await stale(false);
  await expect(review).toBeHidden();

  await owner.getByRole("button", { name: "Members", exact: true }).click();
  await admit("Second connection");
  await rulesTab.click();
  await stale(true);
  await source.selectOption({ label: "Second connection" });
  await fill(buy, bCost);
  await markup.fill("10"); await saved();
  const bWithDefault = await row("Second connection");
  await update(bWithDefault, "save", { source_markup: { expected_revision: "0", percent: decimal("11") } },
    409, "rule_changed");
  await source.selectOption({ label: "Test connection" });

  // A committed review replays by intent after a lost response and a supplier edit.
  const mutation = `${endpoint}/${(await row()).id}`, writes = [];
  const interrupt = async route => {
    writes.push(route.request().postDataJSON());
    if (writes.length === 1) {
      expect((await route.fetch()).status()).toBe(200);
      await route.abort();
    } else await route.continue();
  };
  await owner.route(mutation, interrupt);
  try {
    await review.click();
    await expect(owner.getByRole("alert")).toHaveText(/Could not confirm the change/);
    await draft(inherited);
    const selected = await row();
    const publication = await proof.prepare(mutation, {
      operation_id: crypto.randomUUID(), expected_revision: selected.revision, action: "publish",
    });
    const signed = await proof.assertion(publication);
    await update(await row("Second connection"), "save", { purchase_rates: prices("1", "7", "0.25") });
    expect((await row()).revision).toBe(selected.revision);
    const obsolete = await proof.post(proof.path(publication), signed);
    expect(obsolete.status()).toBe(409);
    expect((await obsolete.json()).error.code).toBe("price_review_stale");
    await review.click();
    await expect(owner.getByRole("alert")).toHaveCount(0);
    await expect(editor.getByText(/Purchase inputs changed/)).toBeVisible();
    expect((await row()).sale_rule.revision).toBe(selected.sale_rule.revision);
    await draft(inherited);
  } finally { await owner.unroute(mutation, interrupt); }
  expect(writes).toHaveLength(2);
  expect(writes[1]).toEqual(writes[0]);
  expect(writes[0].sale_rule).toEqual(apiDefault);
  expect(writes[0]).not.toHaveProperty("rates");
  await update(await row(), "publish", {}, 409, "price_review_stale");
  await update(await row("Second connection"), "save", { purchase_rates: bCost });
  await stale(true);
  await review.click(); await expect(review).toBeHidden();
  await draft(inherited);
  expect(await customerRate()).toBeUndefined();

  for (const theme of ["light", "dark"]) {
    await owner.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
    await owner.reload();
    await rulesTab.click();
    await expect(mode).toHaveValue("api_default");
    await expect(owner.locator(".hosted-entry")).toHaveAttribute("data-theme", theme);
    for (const width of [320, 1024]) {
      await owner.setViewportSize({ width, height: 800 });
      expect(await editor.evaluate(el => document.documentElement.scrollWidth <= innerWidth &&
        [...el.querySelectorAll("button,input,select,dd")].every(field => {
          const { left, right } = field.getBoundingClientRect();
          return left >= 0 && right <= innerWidth;
        }))).toBe(true);
      if (process.env.RELAY_TEST_CATALOG_CAPTURE)
        await editor.screenshot({ path: join(process.env.RELAY_TEST_CATALOG_CAPTURE, `selling-review-${width}-${theme}.png`) });
    }
  }

  // Defaults are connection-wide, even when edited on another model.
  const otherOffer = await row("Test connection", "fixture-text");
  await update(otherOffer, "save", { source_markup: {
    expected_revision: otherOffer.source_markup.revision, percent: null,
  } });
  await stale(true);
  await draft(inherited);
  await mode.selectOption("model_markup");
  await sell.getByLabel("Model markup (%)").fill("50");
  await review.click();
  await draft(custom);
  await expect(review).toBeHidden();
  await source.selectOption({ label: "Test connection" });
  const b = await row("Second connection");
  await update(b, "save", { source_markup: { expected_revision: b.source_markup.revision, percent: null } });
  await stale(false);
  await publish.click();
  await expect(editor.getByText("Published", { exact: true })).toBeVisible();
  const listing = { model, publisher: "openai", protocol: "responses", effective_at_s: expect.any(Number) };
  await expect.poll(customerRate).toEqual({ ...listing, revision: "1", rates: custom });
  const published = await customerRate();

  await openConnection("Test connection");
  const connectionSell = reviewEditor.getByRole("group", { name: "Selling prices" });
  await expect(connectionSell.getByLabel("Selling price rule")).toHaveValue("model_markup");
  await connectionSell.getByLabel("Selling price rule").selectOption("fixed");
  await connectionSell.getByLabel("Output", { exact: true }).fill("");
  await expect(connectionSave).toBeDisabled();
  await expect(reviewEditor.getByRole("button", { name: "Publish to testers" })).toBeDisabled();
  await update((await rules()).find(r => r.model === model && r.enabled), "save", {
    rates: { input: fixed.input, cached: fixed.cached },
  }, 400, "invalid_model_rule");
  await fill(connectionSell, fixed);
  await connectionSave.click();
  await expect(connectionSave).toBeHidden();
  await backToPool();
  await expect(mode).toHaveValue("fixed");
  await expect(editor.getByText("Some selling prices are below purchase cost.")).toBeVisible();
  await draft(fixed);
  expect(await customerRate()).toEqual(published);
  await publish.click();
  await expect.poll(customerRate).toEqual({ ...listing, revision: "2", rates: fixed });
  const fixedListing = await customerRate();
  await checkMemberModels({ owner, customer, endpoint, list, proof, source: (await row()).source_id });
  await checkPoolRotation({ owner, customer, endpoint, proof, list });
  await checkLiveCatalogOrder({ owner, customer, endpoint, list, csrf });
  const latestA = await row(), latestB = await row("Second connection");
  await update(latestA, "save", {
    source_markup: { expected_revision: latestA.source_markup.revision, percent: decimal("25") },
  });
  await update(latestB, "save", {
    source_markup: { expected_revision: latestB.source_markup.revision, percent: decimal("10") },
  });
  await update(await row(), "review", { sale_rule: apiDefault });
  await draft(inherited);
  issue("review-awaits-supplier-removal");
  await wait("review-supplier-removed");
  await stale(true);
  expect((await row("Second connection")).ready).toBe(false);
  await update(await row(), "publish", {}, 409, "price_review_stale");
  expect(await customerRate()).toEqual(fixedListing);
  await update(await row(), "review", { sale_rule: apiDefault });
  await draft(prices("2.5", "3.75", "0.625"));
  expect(await customerRate()).toEqual(fixedListing);
  await owner.reload();
  await rulesTab.click();
  await editor.getByRole("button", { name: "Withdraw model" }).click();
  await editor.getByRole("button", { name: "Confirm withdrawal" }).click();
  await expect.poll(customerRate).toBeUndefined();
  issue("selling-review-checked");
}
