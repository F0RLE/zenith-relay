import { expect, test } from "bun:test";
import { parseConnections } from "../../src/api/connections";

const source = (n: number) => ({
  id: "00000000-0000-0000-0000-" + String(n).padStart(12, "0"), label: null,
  credential_state: "stored", proxy_bound: false, verified_models: [],
  serving: { revision: "0", state: "enabled", in_pool: false, draining: false },
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
test("only a stored source has a revisioned serving state", () => {
  const revoked = { ...source(1), credential_state: "revoked", serving: null };
  const paused = { ...source(2), serving: { revision: "1", state: "paused", in_pool: true, draining: false } };
  const trashed = { ...source(3), serving: { revision: "2", state: "trashed", in_pool: false, draining: true } };
  expect(parseConnections({ items: [revoked, paused, trashed], next_after: null }).items).toEqual([revoked, paused, trashed]);
  for (const change of [{ revision: 1 }, { revision: "0" }, { revision: "01" }, { revision: "1\n" },
    { state: "ready" }, { revision: "9223372036854775808" }, { state: "trashed" },
    { state: "enabled", draining: true }, { draining: "false" }, { in_pool: undefined }]) {
    const serving = { ...paused.serving, ...change };
    expect(() => parseConnections({ items: [{ ...source(1), serving }], next_after: null })).toThrow();
  }
  expect(() => parseConnections({ items: [{ ...source(1), serving: null }], next_after: null })).toThrow();
  expect(() => parseConnections({ items: [{ ...revoked, serving: paused.serving }], next_after: null })).toThrow();
});
test("API and trash pages cannot mix recoverable sources with revoked ones", () => {
  const revoked = { ...source(1), credential_state: "revoked", serving: null };
  const trashed = { ...source(2), serving: { revision: "1", state: "trashed", in_pool: false, draining: false } };
  const page = (row: unknown) => ({ items: [row], next_after: null });
  expect(parseConnections(page(revoked), undefined, "api").items).toEqual([revoked]);
  expect(parseConnections(page(trashed), undefined, "trash").items).toEqual([trashed]);
  expect(() => parseConnections(page(revoked), undefined, "trash")).toThrow();
  expect(() => parseConnections(page(trashed), undefined, "api")).toThrow();
});
test("web connection metadata cannot turn declarations into verified models or expose endpoint credentials", () => {
  const row = { ...source(1), label: "Source", endpoint: "https://fixture.example/v1", declared_models: ["Alpha"] };
  const read = (change: Record<string, unknown>) => parseConnections({ items: [{ ...row, ...change }], next_after: null });
  expect(read({}).items).toEqual([row]);
  for (const change of [{ declared_models: undefined }, { endpoint: "https://fixture.example/v1?secret=key" },
    { endpoint: "https://user:password@fixture.example/v1" }, { endpoint: "http://192.0.2.1:8080/v1" },
    { declared_models: ["Alpha", "alpha"] }, { declared_models: ["model\n"] }]) {
    expect(() => read(change)).toThrow();
  }
});
