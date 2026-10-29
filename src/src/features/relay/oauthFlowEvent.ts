import type { OAuthFlowEvent } from "./api/types";

/** Automation notifications must not rewind an OAuth callback or terminal state. */
export function applyOAuthFlowEvent<T extends OAuthFlowEvent>(flow: T, event: OAuthFlowEvent): T {
  if (flow.loginId !== event.loginId) return flow;
  if (event.automaticLogin !== undefined) return { ...flow, automaticLogin: event.automaticLogin };
  if (["failed", "canceled", "completed", "expired"].includes(flow.status)) return flow;
  if (flow.status === "callback_received" && event.status === "callback_rejected") return flow;
  if (event.diagnostic) {
    // Progress cannot restart completion or overwrite the first failure.
    if (flow.diagnostic?.reason) return flow;
    const stages = ["callback", "token_exchange", "identity_validation", "saving"];
    if (flow.diagnostic && stages.indexOf(event.diagnostic.stage) < stages.indexOf(flow.diagnostic.stage)) return flow;
    return { ...flow, status: event.status === "callback_rejected" ? event.status : flow.status, diagnostic: event.diagnostic };
  }
  if (flow.status === "callback_received" && event.status === "pending") return flow;
  if (flow.status === "callback_rejected" && event.status === "callback_received") return { ...flow, status: event.status, diagnostic: undefined };
  return { ...flow, status: event.status };
}
