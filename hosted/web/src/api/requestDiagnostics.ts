import { record, uuidText } from "./control";
import type { MessageKey } from "../i18n/types";

export const diagnosticStages = {
  dispatch: "diagnostics.stageDispatch", upstream_read: "diagnostics.stageRead",
  protocol_conversion: "diagnostics.stageProtocol", client_delivery: "diagnostics.stageDelivery",
  checkpoint: "diagnostics.stageEvidence", settlement: "diagnostics.stageSettlement", unknown: "diagnostics.unknown",
} as const;
export const diagnosticOrigins = { relay: "diagnostics.originRelay", provider: "diagnostics.originProvider", account: "diagnostics.originAccount" } as const;
export const diagnosticCategories = {
  policy: "diagnostics.categoryPolicy", timeout: "diagnostics.categoryTimeout", transport: "diagnostics.categoryTransport",
  protocol: "diagnostics.stageProtocol", cancellation: "diagnostics.categoryCancellation", accounting: "usage.accounting",
  storage: "diagnostics.categoryStorage", unknown: "diagnostics.unknown",
} as const;
export const diagnosticSeverities = { info: "diagnostics.info", warning: "diagnostics.warning", error: "status.error", critical: "diagnostics.critical" } as const;
export const diagnosticCode = /^[a-z][a-z0-9_]{0,63}$/;
export const diagnosticChoices = { stage: diagnosticStages, origin: diagnosticOrigins, category: diagnosticCategories, severity: diagnosticSeverities } as const;

const messages = [
  "diagnostics.dispatchUnavailable", "diagnostics.dispatchUnconfirmed", "diagnostics.requestPreparation",
  "diagnostics.executionDeadline", "diagnostics.sendUnconfirmed", "diagnostics.upstreamStatus", "diagnostics.readInterrupted",
  "diagnostics.readTimeout", "diagnostics.terminalMissing", "diagnostics.responseLimit", "diagnostics.responseInvalid",
  "diagnostics.upstreamFailed", "diagnostics.streamDeadline", "diagnostics.deliveryClosed", "diagnostics.deliveryBackpressure",
  "diagnostics.evidenceUnavailable", "diagnostics.settlementDeferred",
] as const satisfies readonly MessageKey[];
const fail = (): never => { throw new Error("Invalid request diagnostic"); };
const member = <T extends string>(v: unknown, values: readonly T[]): v is T => typeof v === "string" && values.includes(v as T);
const timestamp = (v: unknown): number =>
  typeof v === "number" && Number.isSafeInteger(v) && v > 0 && v <= 253402300799999 ? v : fail();
const reference = (v: unknown): string => typeof v === "string" && uuidText.test(v) ? v : fail();

export function readOwnerDiagnostic(v: unknown, request: string) {
  if (!record(v) || v.diagnostic_version !== 1 || v.request_id !== request || v.correlation_id !== request
    || v.component !== "hosted_executor" || !member(v.stage, Object.keys(diagnosticStages))
    || !member(v.category, Object.keys(diagnosticCategories)) || !member(v.severity, Object.keys(diagnosticSeverities))
    || (v.origin !== null && !member(v.origin, Object.keys(diagnosticOrigins)))
    || typeof v.code !== "string" || !diagnosticCode.test(v.code)
    || (v.cause_status !== "confirmed" && v.cause_status !== "unknown")
    || typeof v.sequence !== "string" || !/^[1-9]\d{0,18}$/.test(v.sequence) || !record(v.evidence)
    || Object.keys(v.evidence).length !== 1) return fail();
  const status = v.evidence.http_status;
  if (status !== null && (v.code !== "upstream_status" || typeof status !== "number" || !Number.isInteger(status)
    || status < 100 || status > 599)) return fail();
  const happened = v.occurred_at_ms === null ? null : timestamp(v.occurred_at_ms), observed = timestamp(v.observed_at_ms);
  if (timestamp(v.expires_at_ms) < observed || (happened !== null && happened > observed + 30000)) return fail();
  return { id: reference(v.event_id), attempt: v.attempt_id === null ? null : reference(v.attempt_id), code: v.code,
    stage: v.stage as keyof typeof diagnosticStages, origin: v.origin as keyof typeof diagnosticOrigins | null,
    category: v.category as keyof typeof diagnosticCategories, severity: v.severity as keyof typeof diagnosticSeverities,
    summary: member(v.summary_key, messages) ? v.summary_key : "diagnostics.unclassified" as const,
    seq: BigInt(v.sequence), causeUnknown: v.cause_status === "unknown", happened, observed, status };
}

const publicSummaries = {
  settling: "diagnostics.publicSettling", not_dispatched: "diagnostics.publicNotDispatched",
  response_unreadable: "diagnostics.publicUnreadable", interrupted: "diagnostics.publicInterrupted",
  connection_closed: "diagnostics.publicClosed", upstream_failed: "diagnostics.publicFailed",
  model_unavailable: "diagnostics.publicModel", unavailable: "diagnostics.publicUnavailable",
} as const;
const publicCodes = [
  "relay_settlement_pending", "relay_not_dispatched", "relay_upstream_failed", "relay_outcome_pending",
  "relay_request_incomplete", "relay_request_failed", "relay_outcome_unknown",
] as const;

export function readCustomerDiagnostic(v: unknown) {
  if (v === undefined || v === null) return null;
  if (!record(v) || Object.keys(v).length !== 5 || v.diagnostic_version !== 1
    || !member(v.code, publicCodes) || !member(v.summary, Object.keys(publicSummaries))
    || (v.next_action !== "check_status" && v.next_action !== "contact_support")
    || (v.coverage !== "unavailable" && v.coverage !== "partial")
    || (v.coverage === "unavailable") !== (v.summary === "unavailable")
    || (v.code === "relay_settlement_pending") !== (v.summary === "settling")
    || (v.code === "relay_not_dispatched") !== (v.summary === "not_dispatched")
    || (v.code === "relay_upstream_failed") !== (v.summary === "upstream_failed")) return fail();
  return { version: v.diagnostic_version, coverage: v.coverage, code: v.code,
    summary: publicSummaries[v.summary as keyof typeof publicSummaries], action: v.next_action };
}

export type OwnerDiagnostic = ReturnType<typeof readOwnerDiagnostic>;
export type CustomerDiagnostic = NonNullable<ReturnType<typeof readCustomerDiagnostic>>;
