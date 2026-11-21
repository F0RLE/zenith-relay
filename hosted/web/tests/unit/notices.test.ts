import { expect, test } from "bun:test";
import { noticeFields, parseCustomerNotices, parseOwnerNotices } from "../../src/api/notices";

const id = "00000000-0000-4000-8000-000000000001";
const copy = { id, title: "模型更新", message: "Две строки.\n<b>Plain text.</b>" };
const at = Date.parse("2024-04-03T12:00:00Z");

test("notice fields are bounded by the form's UTF-16 lengths and preserve plain text", () => {
  expect(noticeFields("  " + copy.title, copy.message + " ")).toEqual({ title: copy.title, message: copy.message });
  expect(noticeFields("🔔".repeat(60), "X".repeat(4000))).toBeDefined();
  for (const [title, message] of [[" ", "Message"], ["🔔".repeat(61), "Body"], ["Title", "Q".repeat(4001)], ["Title", "X\u0000Y"]])
    expect(noticeFields(title!, message!)).toBeUndefined();
});

test("a draft is owner-only and cannot be rendered as delivered without a receipt", () => {
  const draft = { ...copy, revision: 1, published_at_ms: null, recipient_count: null };
  expect(parseOwnerNotices({ items: [draft] })).toEqual([{ ...copy, revision: 1, publishedAt: null, recipients: null }]);
  expect(() => parseOwnerNotices({ items: [{ ...draft, published_at_ms: at }] })).toThrow();
  expect(() => parseCustomerNotices({ items: [draft] })).toThrow();
  expect(() => parseCustomerNotices({ items: new Array(33).fill({ ...copy, published_at_ms: at, read: false }) })).toThrow();
  expect(parseCustomerNotices({ items: [{ ...copy, published_at_ms: at, read: true }] }))
    .toEqual([{ ...copy, publishedAt: at, read: true }]);
});
