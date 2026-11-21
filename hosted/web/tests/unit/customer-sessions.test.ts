import { expect, test } from "bun:test";
import { parseCustomerSessions } from "../../src/api/customerSessions";

const current = {
  id: "9aa5ec56-2d23-453c-ad96-288e0a359cfd", current: true,
  created_at_s: 1_795_172_400, last_seen_at_s: 1_795_172_520,
};
const other = { ...current, id: "5a07dbe4-3953-40b0-8d03-0b1096b1b084", current: false };

test("browser session list projects only the fields the UI needs", () => {
  expect(parseCustomerSessions({ sessions: [current, other] })).toEqual([
    { id: current.id, current: true, createdAt: 1_795_172_400_000, lastSeenAt: 1_795_172_520_000 },
    { id: other.id, current: false, createdAt: 1_795_172_400_000, lastSeenAt: 1_795_172_520_000 },
  ]);
});

test("browser session list needs one current row first and unique, bounded rows", () => {
  for (const sessions of [[], [other], [other, current], [current, current],
    [current, { ...other, current: true }], [current, { ...other, id: current.id }],
    [current, { ...other, last_seen_at_s: 8_640_000_000_001 }], [current, { ...other, created_at_s: "1" }],
    Array.from({ length: 9 }, () => current)])
    expect(() => parseCustomerSessions({ sessions })).toThrow();
});
