import { expect, test } from "bun:test";
import { parseListings, parseModelDisplayOrder, parseRules, textRate } from "../../src/api/catalog";

const prices = { input: "0", cached: "0.000000000001", output: "20.25" };
const row = { model: "provider/text-v2", publisher: null, rates: prices, revision: "1", protocol: "responses" };
const parked = { id: crypto.randomUUID(), source_id: crypto.randomUUID(), source: "Private connection", model: row.model, proxy: true, revision: "0",
  enabled: false, ready: true, supply_ready: false, admitted: false, purchase: null, effective: null, draft: null,
  serving: { revision: "0", state: "enabled", in_pool: false, draining: false },
  source_markup: { revision: "0", percent: null },
  sale_rule: { revision: "0", mode: "fixed", percent: null, stale: false, below_cost: false } };
const member = { ...parked, admitted: true, serving: { ...parked.serving, in_pool: true } };
test("qualification prices preserve explicit zero and twelve decimal places", () => {
  const [listing] = parseListings([row]);
  expect(listing.rates).toEqual({ input: "0.000000000000", cached: prices.cached, output: "20.250000000000" });
  for (const invalid of ["1e1", "", " ", 0, null, "0.0000000000001", "1000000000000", "-1", "1\n"]) {
    expect(textRate(invalid)).toBeNull();
  }
});
test("invalid or duplicate evidence never becomes a model or a zero price", () => {
  expect(parseListings([])).toEqual([]);
  for (const invalid of [[row,row], [{ ...row, rates: { input: "1" } }], [{ ...row, protocol: "unknown" }],
    [{ ...row, model: "\n" }], Array.from({length:129},(_,i)=>({...row, model:`m${i}`}))]) {
    expect(() => parseListings(invalid)).toThrow();
  }
});
test("publisher metadata is explicit and display-order snapshots contain complete, distinct blocks", () => {
  const revision = "c".repeat(64);
  const first = { model: "gpt-looking", publisher: null };
  const next = { model: "neutral-name", publisher: "sample-publisher" };
  const last = { model: "GPT-LOOKING", publisher: null };
  expect(parseModelDisplayOrder({ revision, items: [next, first, last] }).items).toEqual([next, first, last]);
  expect(parseListings([{ ...row, ...next }, { ...row, ...first }, { ...row, ...last }]).map(item => item.publisher))
    .toEqual(["sample-publisher", null, null]);
  for (const items of [[first, next, last], [first, first], [{ ...first, publisher: "other" }], [{ ...first, publisher: "Vendor\n" }], [{ model: "missing" }]])
    expect(() => parseModelDisplayOrder({ revision, items })).toThrow();
  expect(() => parseModelDisplayOrder({ revision: "1", items: [] })).toThrow();
});
test("parked inventory has neither a tariff nor publication authority", () => {
  expect(parseRules([parked])).toEqual([parked]);
  expect(() => parseRules([{ ...parked, enabled: true, effective: prices }])).toThrow();
  expect(() => parseRules([{ ...parked, effective: prices }])).toThrow();
  expect(() => parseRules([parked,parked])).toThrow();
});
test("rule bindings use the saved source identifier, not a disambiguated label", () => {
  const sameName = { ...parked, id: crypto.randomUUID(), source_id: crypto.randomUUID() };
  expect(parseRules([parked, sameName])).toEqual([parked, sameName]);
  for (const source_id of [undefined, null, parked.source, `${parked.source_id}\n`]) {
    expect(() => parseRules([{ ...parked, source_id }])).toThrow();
  }
});
test("purchase rates must be a complete exact decimal set or explicit null", () => {
  const purchase = { input: "0.000000000000", cached: "0.000000000001", output: "999999999999.123456789012" };
  const active = { ...member, enabled: true, supply_ready: true, purchase, effective: prices };
  expect(parseRules([active])[0]?.purchase).toEqual(purchase);
  for (const invalid of [undefined, prices, { input: purchase.input }, { ...purchase, output: 0 }, { ...purchase, extra: "0" }]) {
    expect(() => parseRules([{ ...member, purchase: invalid }])).toThrow();
  }
});
test("a compatible supplier keeps the publication effective after its original source is lost", () => {
  const live = { ...parked, enabled: true, ready: false, supply_ready: true, effective: prices,
    serving: { revision: "1", state: "trashed", in_pool: false, draining: false } };
  expect(parseRules([live])[0]).toMatchObject({ admitted: false, ready: false, enabled: true, purchase: null, supply_ready: true });
  expect(parseRules([{ ...live, supply_ready: false, effective: null }])[0]?.effective).toBeNull();
  expect(() => parseRules([{ ...live, supply_ready: false }])).toThrow();
  expect(() => parseRules([{ ...live, admitted: true }])).toThrow();
});
test("inherited and custom reviews carry exact markup and never replace published rates", () => {
  const reviewed = { ...member, draft: prices,
    source_markup: { revision: "1", percent: "0.000000000000" },
    sale_rule: { revision: "2", mode: "model_markup", percent: "25.000000000000", stale: true, below_cost: false } };
  const [parsed] = parseRules([reviewed]);
  expect(parsed).toMatchObject({ source_markup: reviewed.source_markup, sale_rule: reviewed.sale_rule, effective: null });
  expect(() => parseRules([{ ...reviewed, draft: null }])).toThrow();
  expect(() => parseRules([{ ...reviewed, sale_rule: { ...reviewed.sale_rule, mode: "api_default" } }])).toThrow();
  expect(() => parseRules([{ ...reviewed, source_markup: { revision: "1", percent: 0 } }])).toThrow();
});
