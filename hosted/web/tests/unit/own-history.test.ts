import { expect, test } from "bun:test";
import { readReceipt, readWalletEntry } from "../../src/api/history";

const id = "8e21c512-ad5c-4feb-b083-237f227193ea";
const row = { request_id: id, key_id: id, model: "example", started_at_s: 1, tariff_revision: "1",
  currency: "USD", precision: 6, execution: "completed", accounting: "settled", basis: "provider_final",
  input_tokens: "40000", cached_tokens: "10000", output_tokens: "0", total_tokens: "40000", hold_micro_usd: "2000000",
  charge_micro_usd: "0", wallet_receipt_id: id, credit: null };

test("known zero, unknown and large exact receipts remain distinct", () => {
  expect(readReceipt(row)).toMatchObject({input:40000n,cached:10000n,output:0n,charge:0n});
  expect(readReceipt({...row,execution:"uncertain",accounting:"pending",basis:null,
    output_tokens:null,total_tokens:null,charge_micro_usd:null,wallet_receipt_id:null})).toMatchObject({charge:null,output:null,total:null});
  expect(readReceipt({...row,hold_micro_usd:"9223372036854775807"}).hold).toBe(9223372036854775807n);
});

test("malformed or contradictory financial evidence is not displayed", () => {
  for (const change of [{cached_tokens:"40001"},{charge_micro_usd:"2000001"},
    {basis:"not_sent"},{accounting:"pending"},{wallet_receipt_id:null},{tariff_revision:"0"},
    {hold_micro_usd:"9223372036854775808"},{charge_micro_usd:0},{total_tokens:"50000"}]) {
    expect(() => readReceipt({...row,...change})).toThrow();
  }
});

test("a verified credit is linked to the original charge but is a separate wallet entry", () => {
  const creditId="ce96f353-60a4-493a-9075-6d6aae946892";
  const credit={wallet_receipt_id:creditId,amount_micro_usd:"180000"};
  const partial={...row,basis:"provider_partial",charge_micro_usd:"1200000",credit};
  expect(readReceipt(partial)).toMatchObject({charge:1200000n,credit:{id:creditId,amount:180000n}});
  for (const change of [
    {basis:"provider_final"},{charge_micro_usd:"179999"},{credit:{...credit,amount_micro_usd:"0"}},
    {credit:{...credit,wallet_receipt_id:id}},
  ]) expect(() => readReceipt({...partial,...change})).toThrow();

  const entry={id:creditId,kind:"request_credit",posted_at_s:2,request_id:id,refund_id:null,amount_micro_usd:"180000"};
  expect(readWalletEntry(entry)).toMatchObject({type:"request_credit",requestId:id,amount:180000n});
  expect(() => readWalletEntry({...entry,request_id:null})).toThrow();
  expect(() => readWalletEntry({...entry,amount_micro_usd:"0"})).toThrow();
  const refund = { ...entry, kind: "refund_paid", request_id: null, refund_id: id, amount_micro_usd: "2000000" };
  expect(readWalletEntry(refund)).toMatchObject({ type: "refund_paid", refundId: id, requestId: null, amount: 2_000_000n });
  expect(() => readWalletEntry({ ...refund, request_id: id })).toThrow();
  expect(() => readWalletEntry({ ...refund, refund_id: null })).toThrow();
});
