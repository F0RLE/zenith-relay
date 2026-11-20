import { expect, test } from "bun:test";
import { defaultOwnerUsage, ownerRequestPath, ownerUsageHref, parseOwnerRequest,
  parseOwnerUsagePage, parseOwnerUsageView } from "../../src/api/ownerUsage";

const request = "00000000-0000-4000-8000-000000000001", workspace = "00000000-0000-4000-8000-000000000002";
const key = "00000000-0000-4000-8000-000000000003", source = "00000000-0000-4000-8000-000000000004";
const from = 1709251200123, to = from + 3_600_000;

test("owner chart links preserve exact millisecond bounds, filters and safe request references", () => {
  const view = { errors: true, window: { from, to }, model: "example/a:fast", workspace, key, source, accounting: "pending" } as const;
  const href = new URL(ownerUsageHref(view, request), "http://localhost:1443");
  expect(ownerRequestPath(href.pathname)).toBe(request);
  expect(parseOwnerUsageView(href.search)).toMatchObject(view);
  expect(parseOwnerUsageView("")?.window).toEqual(defaultOwnerUsage.window);
  for (const query of ["hours=0", "tab=customer", "hours=168&hours=24", `from_ms=${to}&to_ms=${from}`,
    `from_ms=${from}`, `from_ms=${from}&to_ms=${to}&hours=24`, "model=not+a+model", "source_id=secret", "report=customer"])
    expect(parseOwnerUsageView(query)).toBeNull();
  expect(ownerRequestPath("/admin/usage/requests/" + request + "/extra")).toBeNull();
  expect(ownerRequestPath("/app/usage/requests/" + request)).toBeNull();
});

const heldRow = {
  request_id: request, model: "example/a:fast", started_at_ms: from, workspace_id: workspace, key_id: key,
  source_id: source, execution: "completed", accounting: "pending", issues: ["settlement"], posted_micro_usd: null,
};
const list = { version: 1, from_ms: from, to_ms: to, as_of_ms: to + 1, errors: true, items: [heldRow], next_before: null };

test("completed request with unsettled money is one error row; its charge is not zero", () => {
  const view = { errors: true, window: { from, to }, key };
  expect(parseOwnerUsagePage(list, view).items[0]).toMatchObject({ posted: null, issues: ["settlement"], execution: "completed" });
  for (const row of [{ ...heldRow, issues: [] }, { ...heldRow, posted_micro_usd: "0" },
    { ...heldRow, started_at_ms: to }, { ...heldRow, key_id: workspace }, { ...heldRow, execution: "uncertain" }])
    expect(() => parseOwnerUsagePage({ ...list, items: [row] }, view)).toThrow();
  expect(() => parseOwnerUsagePage({ ...list, next_before: request }, view)).toThrow();
  expect(() => parseOwnerUsagePage({ ...list, items: [heldRow, heldRow] }, view)).toThrow();
});

test("owner detail keeps purchased valuation separate and fails closed on fabricated attempt stages", () => {
  const evidence = {
    version: 1, as_of_ms: to, workspace_id: workspace, streaming: true, issues: ["settlement"],
    receipt: { ...heldRow, started_at_s: Math.floor(from / 1000), tariff_revision: "2", currency: "USD",
      precision: 6, input_tokens: "0", output_tokens: null, cached_tokens: null, total_tokens: null,
      hold_micro_usd: "9223372036854775807", charge_micro_usd: null, credit: null, basis: null, wallet_receipt_id: null,
      usage_quality: "partial" },
    attempt: { id: key, source_id: source, purchase_revision: "1", valued_micro_usd: "9007199254740993", terminal: "completed" },
    timeline: [{ event: "reserved", at_ms: from }, { event: "dispatch_permitted", at_ms: from + 5 },
      { event: "terminal_observed", at_ms: from + 10 }],
  };
  const detail = parseOwnerRequest(evidence, request);
  expect(detail.attempt?.value).toBe(9007199254740993n);
  expect(detail.receipt.charge).toBeNull();
  expect(detail.receipt.hold).toBe(9223372036854775807n);
  expect(() => parseOwnerRequest({ ...evidence, attempt: null }, request)).toThrow();
  expect(() => parseOwnerRequest({ ...evidence, timeline: [...evidence.timeline,
    { event: "credit_posted", at_ms: from + 20 }] }, request)).toThrow();
  expect(() => parseOwnerRequest(evidence, workspace)).toThrow();
});
