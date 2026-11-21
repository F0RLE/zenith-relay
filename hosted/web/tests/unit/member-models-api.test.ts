import { expect, test } from "bun:test";
import { parseMemberModels } from "../../src/api/memberModels";

const snapshot = {
  scope_id: "db8e2c08-8cc5-4ac2-8db2-723b85864dd2",
  source_revision: "0",
  revision: "1",
  items: [{ model: "Exact/Model", allowed: true }, { model: "exact/model", allowed: false }],
};

test("member model policy reads exact IDs and explicit allowed states", () => {
  expect(parseMemberModels(snapshot)).toEqual(snapshot);
});

test("invalid, repeated or unordered model policies cannot become editable", () => {
  for (const view of [
    { ...snapshot, scope_id: null },
    { ...snapshot, scope_id: "00000000-0000-0000-0000-000000000000" },
    { ...snapshot, revision: "9223372036854775808" },
    { ...snapshot, source_revision: "01" },
    { ...snapshot, items: [] },
    { ...snapshot, items: [snapshot.items[0], snapshot.items[0]] },
    { ...snapshot, items: [...snapshot.items].reverse() },
    { ...snapshot, items: [{ model: "model with spaces", allowed: false }] },
    { ...snapshot, items: [{ model: "exact", allowed: null }] },
  ]) expect(() => parseMemberModels(view)).toThrow();
});
