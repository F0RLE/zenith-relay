import type {
  DefaultServiceTier,
  ErrorOrigin,
  LocalUsage,
  ObservedServiceTier,
  ReasoningEffort,
  RoutingDiagnostics,
  ToolUseDiagnostics,
  UpstreamErrorDetails,
  UsageTransport,
  UsageTotals,
} from "../../api/types";
import type { TokenSpeedSample } from "../../usageSpeed";
import { totalsFromUsageSamples, type UsageTotalsSample } from "../../usageTotals";
import { modelIdForDisplay } from "./usageModelIdentity";

export type CodexRequestOrigin =
  | "activity_summary"
  | "task_title"
  | "blocked_activity_summary"
  | "blocked_task_title"
  | "memory_extraction"
  | "memory_consolidation"
  | "blocked_memory_extraction"
  | "blocked_memory_consolidation"
  | null;

export type UsageRow = {
  id: string | number;
  attempt: number;
  time: string;
  success: boolean;
  model: string | null;
  requestedModel: string | null;
  routedModel: string | null;
  requestedReasoningEffort: ReasoningEffort | null;
  effectiveReasoningEffort: ReasoningEffort | null;
  connection: string;
  wireApi: string | null;
  transport: UsageTransport;
  serviceTier: DefaultServiceTier | null;
  appliedServiceTier: ObservedServiceTier | null;
  ttft: number | null;
  generationMs: number | null;
  duration: number;
  inputTokens: number | null;
  cachedInputTokens: number | null;
  cacheWriteInputTokens: number | null;
  cacheWriteTtl: string | null;
  clientContextId: string | null;
  reasoningTokens: number | null;
  outputTokens: number | null;
  tokens: number | null;
  requestId: string | null;
  httpStatus: number | null;
  errorCategory: string | null;
  errorOrigin: ErrorOrigin | null;
  upstreamError?: UpstreamErrorDetails | null;
  toolUse: ToolUseDiagnostics | null;
  routing: RoutingDiagnostics | null;
  accountId: string | null;
  candidateKind: "account" | "source";
  candidateKey: string;
  apiEquivalent: UsageTotals["apiEquivalent"] | null;
  apiEquivalentBreakdown?: LocalUsage["apiEquivalentBreakdown"];
  requestOrigin: CodexRequestOrigin;
};

/** Retains a safe raw tier reported by an upstream response for diagnostics. */
export function normalizeObservedServiceTier(serviceTierValue: unknown): ObservedServiceTier | null {
  if (typeof serviceTierValue !== "string") return null;
  const tier = serviceTierValue.trim().toLowerCase();
  return /^[a-z0-9_-]{1,48}$/.test(tier) ? tier : null;
}

export function codexRequestOriginFromErrorCategory(category: string | null): CodexRequestOrigin {
  if (category === "codex_activity_summary") return "activity_summary";
  if (category === "codex_task_title") return "task_title";
  if (category === "codex_background_blocked_activity_summary") return "blocked_activity_summary";
  if (category === "codex_background_blocked_task_title") return "blocked_task_title";
  if (category === "codex_memory_extraction") return "memory_extraction";
  if (category === "codex_memory_consolidation") return "memory_consolidation";
  if (category === "codex_background_blocked_memory_extraction") return "blocked_memory_extraction";
  if (category === "codex_background_blocked_memory_consolidation") return "blocked_memory_consolidation";
  return null;
}

type UsageEvent = LocalUsage;

type UsageRowLabels = {
  backgroundConnection: string;
  unknownAccount: string;
  removedAccount: string;
  unknownConnection: string;
};

export type LocalUsageRowLabels = UsageRowLabels & {
  accountLabels: ReadonlyMap<string, string>;
  sourceLabels: ReadonlyMap<string, string>;
};

function usageRowFromEvent(
  event: UsageEvent,
  time: string,
  connection: string,
  accountId: string | null,
  candidateKind: UsageRow["candidateKind"],
  candidateKey: string,
  requestOrigin: CodexRequestOrigin,
): UsageRow {
  return {
    id: event.id,
    attempt: event.attempt,
    time,
    success: event.success,
    model: modelIdForDisplay(event.resolvedModel ?? event.requestedModel),
    requestedModel: event.requestedModel,
    routedModel: event.resolvedModel,
    requestedReasoningEffort: event.requestedReasoningEffort ?? null,
    effectiveReasoningEffort: event.effectiveReasoningEffort ?? null,
    connection,
    wireApi: event.wireApi,
    transport: event.transport ?? "http",
    serviceTier: event.serviceTier ?? null,
    appliedServiceTier: normalizeObservedServiceTier(event.appliedServiceTier),
    ttft: event.ttftMs ?? null,
    generationMs: event.generationMs ?? null,
    duration: event.latencyMs,
    inputTokens: event.inputTokens,
    cachedInputTokens: event.cachedInputTokens,
    cacheWriteInputTokens: event.cacheWriteInputTokens ?? null,
    cacheWriteTtl: event.cacheWriteTtl ?? null,
    reasoningTokens: event.reasoningTokens,
    outputTokens: event.outputTokens,
    tokens: event.totalTokens,
    requestId: event.requestId,
    httpStatus: event.httpStatus,
    errorCategory: event.errorCategory,
    errorOrigin: event.errorOrigin ?? null,
    upstreamError: event.success ? null : event.upstreamError ?? null,
    toolUse: event.toolUse ?? null,
    routing: event.routing ?? null,
    accountId,
    candidateKind,
    candidateKey,
    apiEquivalent: event.apiEquivalent ?? null,
    apiEquivalentBreakdown: event.apiEquivalentBreakdown,
    requestOrigin,
    clientContextId: "clientContextId" in event ? event.clientContextId ?? null : null,
  };
}

/** Builds UI rows from persisted local telemetry. */
export function usageRowsFromLocal(events: readonly LocalUsage[], labels: LocalUsageRowLabels): UsageRow[] {
  return events.map((event) => {
    const requestOrigin = codexRequestOriginFromErrorCategory(event.errorCategory);
    const connection = requestOrigin
      ? labels.backgroundConnection
      : event.accountId
        ? labels.accountLabels.get(event.accountId) ?? labels.removedAccount
        : labels.sourceLabels.get(event.sourceId) ?? labels.unknownConnection;
    return usageRowFromEvent(
      event,
      event.createdAt,
      connection,
      event.accountId ?? null,
      event.accountId ? "account" : "source",
      event.accountId ?? event.sourceId,
      requestOrigin,
    );
  });
}

/** Reports show the latest result of each Relay request, never its intermediate failures. */
export function logicalRequestRows(rows: readonly UsageRow[]): UsageRow[] {
  const requests = new Map<string, UsageRow>();
  for (const row of rows) {
    const key = row.requestId ? `request:${row.requestId}` : `record:${row.id}`;
    const previous = requests.get(key);
    if (!previous || row.attempt > previous.attempt
      || (row.attempt === previous.attempt && Date.parse(row.time) > Date.parse(previous.time))) {
      requests.set(key, row);
    }
  }
  return [...requests.values()].sort((left, right) => Date.parse(right.time) - Date.parse(left.time));
}

export function totalsFromRows(rows: UsageRow[]): UsageTotals {
  const uniqueSends = physicalRequestRows(rows);
  const sends = totalsFromUsageSamples(uniqueSends.map(usageTotalsSampleFromRow));
  const requests = totalsFromUsageSamples(logicalRequestRows(uniqueSends).map(usageTotalsSampleFromRow));
  return { ...requests,
    inputTokens: sends.inputTokens, cachedInputTokens: sends.cachedInputTokens,
    cachedInputSamples: sends.cachedInputSamples, cacheWriteInputTokens: sends.cacheWriteInputTokens ?? 0,
    cacheWriteInputSamples: sends.cacheWriteInputSamples ?? 0, reasoningTokens: sends.reasoningTokens,
    outputTokens: sends.outputTokens, totalTokens: sends.totalTokens, apiEquivalent: sends.apiEquivalent,
  };
}

/** Each upstream send is visible once; repeated observations replace that send. */
export function physicalRequestRows(rows: readonly UsageRow[]): UsageRow[] {
  const sends = new Map<string, UsageRow>();
  for (const row of rows) {
    const key = row.requestId ? `${row.requestId}:${row.attempt}` : `record:${row.id}`;
    const previous = sends.get(key);
    if (!previous || Date.parse(row.time) >= Date.parse(previous.time)) sends.set(key, row);
  }
  return [...sends.values()].sort((a, b) => Date.parse(b.time) - Date.parse(a.time) || b.attempt - a.attempt);
}

function usageTotalsSampleFromRow(row: UsageRow): UsageTotalsSample {
  return {
    success: row.success,
    latencyMs: row.duration,
    ttftMs: row.ttft,
    generationMs: row.generationMs,
    inputTokens: row.inputTokens,
    cachedInputTokens: row.cachedInputTokens,
    cacheWriteInputTokens: row.cacheWriteInputTokens,
    reasoningTokens: row.reasoningTokens,
    outputTokens: row.outputTokens,
    totalTokens: row.tokens,
    apiEquivalent: row.apiEquivalent,
  };
}

export function usageSpeedSample(row: UsageRow): TokenSpeedSample {
  return {
    success: row.success,
    outputTokens: row.outputTokens,
    reasoningTokens: row.reasoningTokens,
    durationMs: row.generationMs,
  };
}

