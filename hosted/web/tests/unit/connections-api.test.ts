import { expect, test } from "bun:test";
import { parseConnections } from "../../src/api/connections";

const source = (n: number) => ({
  id: "00000000-0000-0000-0000-" + String(n).padStart(12, "0"), label: null,
  credential_state: "stored", proxy_bound: false, verified_models: [],
});
test("a saved source is visible without a label, proxy or current model observations", () => {
  const page = { items: [source(1)], next_after: null };
  expect(parseConnections(page)).toEqual(page);
  expect(parseConnections({ items: [], next_after: null }, source(1).id).items).toEqual([]);
});
test("connection pages cannot repeat rows or advance past unreturned sources", () => {
  const rows = Array.from({ length: 32 }, (_, i) => source(i + 1));
  expect(parseConnections({ items: rows, next_after: rows[31].id }).next_after).toBe(rows[31].id);
  for (const [items, next] of [[rows, rows[0].id], [[rows[0]], rows[0].id], [[rows[0],rows[0]], null],
    [[rows[1],rows[0]], null], [[...rows,source(33)], null]]) {
    expect(() => parseConnections({ items, next_after: next })).toThrow();
  }
  expect(() => parseConnections({ items: rows, next_after: null }, rows[0].id)).toThrow();
});
test("invalid observation or binding data is not rendered as verified inventory", () => {
  const active = { ...source(2), label: "Reviewed source", proxy_bound: true, verified_models: ["A","a"] };
  expect(parseConnections({ items: [active], next_after: null }).items).toEqual([active]);
  for (const change of [{ label: "\nname" }, { credential_state: "revoked" }, { credential_state: "online" }, { proxy_bound: 1 },
    { verified_models: ["model\n"] }, { verified_models: ["a","a"] }, { verified_models: ["a","A"] }, { verified_models: null }]) {
    expect(() => parseConnections({ items: [{ ...active, ...change }], next_after: null })).toThrow();
  }
});
