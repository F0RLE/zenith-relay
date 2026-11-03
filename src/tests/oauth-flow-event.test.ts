import { expect, test } from "bun:test";
import { applyOAuthFlowEvent } from "../src/features/relay/oauthFlowEvent";
import type { OAuthFlowEvent } from "../src/features/relay/api/types";

test("automation stops never rewind callback or terminal OAuth status", () => {
  for (const status of ["pending", "callback_received", "failed", "canceled", "completed", "expired", "callback_rejected"] as const) {
    const flow = { loginId: "synthetic", status, automaticLogin: true };
    const stopped = applyOAuthFlowEvent(flow, { loginId: flow.loginId, status: "pending", automaticLogin: false });
    expect(stopped.status).toBe(status);
    expect(stopped.automaticLogin).toBe(false);
  }
});

test("early normal events keep the latest automation state", () => {
  let early: OAuthFlowEvent = { loginId: "synthetic", status: "pending", automaticLogin: false };
  early = applyOAuthFlowEvent(early, { loginId: early.loginId, status: "callback_received" });
  expect(early).toEqual({ loginId: "synthetic", status: "callback_received", automaticLogin: false });
  early = applyOAuthFlowEvent(early, { loginId: early.loginId, status: "pending", automaticLogin: false });
  expect(early.status).toBe("callback_received");
});

test("OAuth diagnostics preserve status, stage ordering, and the first failure", () => {
  let flow: OAuthFlowEvent = { loginId: "synthetic", status: "callback_received" };
  flow = applyOAuthFlowEvent(flow, { ...flow, diagnostic: { stage: "token_exchange", elapsedMs: 0 } });
  expect(flow.status).toBe("callback_received");
  flow = applyOAuthFlowEvent(flow, { ...flow, diagnostic: { stage: "identity_validation", elapsedMs: 20, reason: "identity_invalid" } });
  const failed = flow;
  flow = applyOAuthFlowEvent(flow, { ...flow, diagnostic: { stage: "saving", elapsedMs: 0 } });
  expect(flow).toBe(failed);
  expect(applyOAuthFlowEvent(flow, { loginId: "other", status: "completed" })).toBe(flow);
  const terminal: OAuthFlowEvent = { ...flow, status: "failed" };
  expect(applyOAuthFlowEvent(terminal, { loginId: flow.loginId, status: "callback_received" })).toBe(terminal);
});

test("a valid callback can recover a rejected callback without stale diagnostics", () => {
  const pending: OAuthFlowEvent = { loginId: "synthetic", status: "pending" };
  const rejected = applyOAuthFlowEvent(pending, { ...pending, status: "callback_rejected", diagnostic: { stage: "callback", elapsedMs: 2, reason: "callback_invalid" } });
  expect(rejected.status).toBe("callback_rejected");
  const accepted = applyOAuthFlowEvent(rejected, { loginId: pending.loginId, status: "callback_received" });
  expect(accepted.status).toBe("callback_received");
  expect(accepted.diagnostic).toBeUndefined();
  expect(applyOAuthFlowEvent(accepted, rejected)).toBe(accepted);
});
