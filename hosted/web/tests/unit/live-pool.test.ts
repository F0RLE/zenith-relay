import { expect, test } from "bun:test";
import type { Rule } from "../../src/api/catalog";
import { parsePoolActivity } from "../../src/api/poolActivity";
import { parsePoolRotation } from "../../src/api/poolRotation";
import { inventoryForPool, liveMemberOrder, livePoolActivity } from "../../src/features/owner/livePool";

const source = crypto.randomUUID(), other = crypto.randomUUID();
const binding = (fields: Partial<Rule> = {}): Rule => ({
  id: crypto.randomUUID(), source_id: source, source: "API source", model: "Fixture-text", proxy: true,
  revision: "1", admitted: true, enabled: false, ready: true, supply_ready: false, purchase: null, draft: null,
  effective: null, source_markup: { revision: "0", percent: null },
  sale_rule: { revision: "0", mode: "fixed", percent: null, stale: false, below_cost: false },
  serving: { revision: "0", state: "enabled", in_pool: true, draining: false }, ...fields,
});

test("members are physical sources, not model bindings or shared labels", () => {
  const checked = binding({ source_id: other, admitted: false, model: "ready" });
  const { members, candidates } = inventoryForPool([
    binding(), binding(), binding({ model: "fixture-text" }), binding({ source_id: other }), checked,
    binding({ admitted: false, ready: false }), binding({ admitted: false, serving: null }),
    binding({ admitted: false, serving: { revision: "2", state: "paused", in_pool: false, draining: false } }),
    binding({ admitted: false, enabled: true }),
  ]);
  expect(members.map(member => [member.id, member.models])).toEqual([
    [source, ["Fixture-text", "fixture-text"]], [other, ["Fixture-text"]],
  ]);
  expect(candidates).toEqual([checked]);
});

test("activity keeps observed IDs and includes work draining after a member leaves", () => {
  const { members } = inventoryForPool([binding()]);
  const model = "Fixture-text";
  const view = parsePoolActivity({ strategy: "automatic", member_order: [], items: [
    { source_id: source, source: "Older label", model, requests: 1 },
    { source_id: other, source: "Parked source", model: "fixture-text", requests: 1 },
  ] });
  expect(livePoolActivity(view, members)).toEqual({
    total: 2, observed: true, current: [{ name: "API source" }, { name: "Parked source" }],
    requests: new Map([[source, 1], [other, 1]]),
    models: [{ id: model, requests: 1 }, { id: "fixture-text", requests: 1 }],
  });
  expect(livePoolActivity(undefined, members)).toMatchObject({ observed: false, total: 0, current: [] });
  expect(livePoolActivity(parsePoolActivity({ strategy: "automatic", member_order: [], items: [] }), members)).toMatchObject({ observed: true, total: 0, current: [] });
});

test("invalid or repeated dispatch data fails closed", () => {
  const row = { source_id: source, source: "API source", model: "Fixture-text", requests: 1 };
  for (const view of [
    { strategy: "round_robin", items: [] }, { strategy: null, items: [row] },
    { strategy: "automatic", items: [row, row] },
    { strategy: "automatic", member_order: [source], items: [] },
    { strategy: "failover", member_order: [source, source], items: [] },
    ...[0, -1, NaN, "1", 0.5, 129].map(requests => ({ strategy: "automatic", items: [{ ...row, requests }] })),
    { strategy: "automatic", items: [{ ...row, model: "Fixture-text\n" }] },
  ]) expect(() => parsePoolActivity({ member_order: [], ...view })).toThrow();
});

test("saved order ranks only members, with new or unranked connections last", () => {
  const { members } = inventoryForPool([binding(), binding({ source_id: other })]);
  const state = parsePoolActivity({ strategy: "failover", member_order: [crypto.randomUUID(), other], items: [] });
  expect(liveMemberOrder(members, state).map(member => member.id)).toEqual([other, source]);
  expect(livePoolActivity(state, members).current).toEqual([]);
});

test("live rotation exposes only backed modes and rejects invalid policy snapshots", () => {
  const member = { source_id: source, label: "Member", paused: true };
  const snapshot = { scope_id: crypto.randomUUID(), revision: "a".repeat(64), mode: "failover", members: [member] };
  expect(parsePoolRotation(snapshot)).toEqual(snapshot);
  for (const corrupted of [{ mode: "round_robin" }, { members: [member, member] }, { revision: 1 },
    { members: [{ ...member, paused: "false" }] }, { members: [{ ...member, label: "\n" }] }]) {
    expect(() => parsePoolRotation({ ...snapshot, ...corrupted })).toThrow();
  }
});
