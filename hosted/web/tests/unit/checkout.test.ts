import { expect, test } from "bun:test";
import { parseCreditInput, parseResumedTopUp } from "../../src/api/checkout";
import type { TopUp } from "../../src/api/checkout";

test("wallet credit input converts exact cents without floating-point amounts", () => {
  for (const [input, expected] of [["10", 10_000_000n], ["10.5", 10_500_000n], [" 10,05 ", 10_050_000n],
    ["0.01", 10_000n], ["10000.00", 10_000_000_000n]] as const) expect(parseCreditInput(input)).toBe(expected);
  for (const input of ["", "-5", "1.005", "1e3", "$10", "1,000", "1.2.3", "Infinity"]) expect(parseCreditInput(input)).toBeNull();
});

test("resuming validates the original payment and rejects inconsistent handoffs", () => {
  const id = "8e21c512-ad5c-4feb-b083-237f227193ea", receipt = "ce96f353-60a4-493a-9075-6d6aae946892";
  const original: TopUp = { id, operation: id, quote: id, label: "Card", at: 1,
    amount: 10_000_000n, total: 10_500_000n, state: "awaiting_payment", receipt: null, canResume: true,
    refundable: 0n, refund: null };
  const row = { id, operation_id: id, quote_id: id, label: "Card", created_at_s: 1, currency: "USD",
    credit_micro_usd: "10000000", payable_minor: "1050", state: "awaiting_payment", receipt_id: null,
    can_resume: true, refundable_micro_usd: "0", refund: null };
  const target = "https://checkout.merchant.invalid/invoice/fixture";
  expect(parseResumedTopUp({ payment: row, handoff_url: target }, original)).toMatchObject({ handoff: target });
  const paid = { ...row, state: "paid", receipt_id: receipt, can_resume: false, refundable_micro_usd: "10000000" };
  expect(parseResumedTopUp({ payment: paid, handoff_url: null }, original)).toMatchObject({
    handoff: null, payment: { state: "paid", receipt, canResume: false, refundable: 10_000_000n },
  });
  const withRefund = { ...paid, refundable_micro_usd: "8000000", refund: { id: receipt, payment_id: id, state: "queued",
    amount_micro_usd: "2000000", requested_at_s: 1, receipt_id: null } };
  expect(parseResumedTopUp({ payment: withRefund, handoff_url: null }, original).payment.refund?.amount).toBe(2_000_000n);
  for (const change of [{ refundable_micro_usd: "10000000" }, { refund: { ...withRefund.refund, payment_id: receipt } },
    { refund: { ...withRefund.refund, state: "succeeded" } }])
    expect(() => parseResumedTopUp({ payment: { ...withRefund, ...change }, handoff_url: null }, original)).toThrow();
  for (const patch of [{ id: receipt }, { operation_id: receipt }, { quote_id: receipt },
    { payable_minor: "1051" }, { currency: "EUR" }, { can_resume: false }, paid])
    expect(() => parseResumedTopUp({ payment: { ...row, ...patch }, handoff_url: target }, original)).toThrow();
  for (const handoff_url of [undefined, "//checkout.merchant.invalid/", "javascript:void(0)", target + "#",
    "https://user@checkout.merchant.invalid/"])
    expect(() => parseResumedTopUp({ payment: row, handoff_url }, original)).toThrow();
});
