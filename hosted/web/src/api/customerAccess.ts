import { ControlError, control, record, uuidText } from "./control";
import { confirmWithPasskey } from "./ownerSession";

export const accessStates = { active: "access.active", suspended: "access.suspended", closed: "access.closed" } as const;
export const accessActions = { suspend: "access.suspend", reinstate: "access.reinstate", close: "access.close" } as const;
export const accessReasons = {
  security_review: "access.securityReview", policy_review: "access.policyReview",
  review_complete: "access.reviewComplete", verified_customer_request: "access.verifiedRequest",
} as const;
export type AccessAction = keyof typeof accessActions;
export type AccessReason = keyof typeof accessReasons;
type AccessState = keyof typeof accessStates;
export type AccessChange = {
  operation_id: string; expected_revision: string; action: AccessAction; reason_code: AccessReason;
};

const actionReasons: Record<AccessAction, readonly AccessReason[]> = {
  suspend: ["security_review", "policy_review"], reinstate: ["review_complete"], close: ["verified_customer_request"],
};
export const reasonsFor = (action: AccessAction) => actionReasons[action];
export const customerCasePath = (workspace: string) => `/admin/customers/${workspace}`;
const endpoint = (workspace: string) => `admin/customers/${workspace}`;

export async function readCustomerCase(workspace: string, signal: AbortSignal) {
  const data = await control(endpoint(workspace), { signal });
  if (data.workspace_id !== workspace || typeof data.revision !== "string" || !/^[1-9]\d{0,18}$/.test(data.revision)
    || typeof data.state !== "string" || !Object.hasOwn(accessStates, data.state)) throw new Error("Invalid customer case");
  const change = data.last_change;
  let last: { id: string; action: AccessAction; reason: AccessReason; at: number } | null = null;
  if (change !== null) {
    if (!record(change) || typeof change.operation_id !== "string" || !uuidText.test(change.operation_id)
      || typeof change.action !== "string" || !Object.hasOwn(accessActions, change.action)
      || typeof change.reason_code !== "string" || !Object.hasOwn(accessReasons, change.reason_code)
      || !reasonsFor(change.action as AccessAction).includes(change.reason_code as AccessReason)
      || typeof change.applied_at_ms !== "number" || !Number.isSafeInteger(change.applied_at_ms)
      || change.applied_at_ms <= 0 || change.applied_at_ms >= 253402300800000) throw new Error("Invalid access receipt");
    last = { id: change.operation_id, action: change.action as AccessAction,
      reason: change.reason_code as AccessReason, at: change.applied_at_ms };
  }
  return { state: data.state as AccessState, revision: data.revision, last };
}

function receipt(data: Record<string, unknown>, workspace: string, command: AccessChange) {
  return data.applied === true && data.workspace_id === workspace && data.operation_id === command.operation_id
    && data.revision === (BigInt(command.expected_revision) + 1n).toString() ? true : undefined;
}

export function confirmCustomerChange(csrf: string, workspace: string, command: AccessChange) {
  return confirmWithPasskey(endpoint(workspace), csrf, command, "admin/customers/confirmations",
    data => receipt(data, workspace, command));
}

export async function checkCustomerChange(csrf: string, workspace: string, command: AccessChange) {
  try {
    const data = await control(endpoint(workspace), { method: "POST",
      headers: { "Content-Type": "application/json", "X-Relay-CSRF": csrf }, body: JSON.stringify(command) });
    if (!receipt(data, workspace, command)) throw new Error("Invalid access receipt");
    return true;
  } catch (e) {
    if (e instanceof ControlError && e.code === "fresh_passkey_required") return false;
    throw e;
  }
}
