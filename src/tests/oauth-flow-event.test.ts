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
