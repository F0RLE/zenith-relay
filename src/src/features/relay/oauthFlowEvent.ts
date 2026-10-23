import type { OAuthFlowEvent } from "./api/types";

/** Automation notifications must not rewind an OAuth callback or terminal state. */
export function applyOAuthFlowEvent<T extends OAuthFlowEvent>(flow: T, event: OAuthFlowEvent): T {
  return event.automaticLogin === undefined
    ? { ...flow, status: event.status }
    : { ...flow, automaticLogin: event.automaticLogin };
}
