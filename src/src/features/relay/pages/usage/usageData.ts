import type {
  CacheWriteTtl,
  DefaultServiceTier,
  ErrorOrigin,
  LocalUsage,
  ObservedServiceTier,
  ReasoningEffort,
  RemoteUsage,
  RoutingDiagnostics,
  ToolUseDiagnostics,
  UsageTotals,
} from "../../api/types";
import type { TokenSpeedSample } from "../../usageSpeed";
import { totalsFromUsageSamples, type UsageTotalsSample } from "../../usageTotals";

export type CodexRequestOrigin =
  | "activity_summary"
  | "task_title"
  | "blocked_activity_summary"
  | "blocked_task_title"
  | null;

export type UsageRow = {
  id: string | number;
  attempt: number;
  time: string;
  success: boolean;
  model: string | null;
  requestedReasoningEffort: ReasoningEffort | null;
  effectiveReasoningEffort: ReasoningEffort | null;
  connection: string;
  wireApi: string | null;
  serviceTier: DefaultServiceTier | null;
  appliedServiceTier: ObservedServiceTier | null;
  ttft: number | null;
  generationMs: number | null;
  duration: number;
  inputTokens: number | null;
  cachedInputTokens: number | null;
  cacheWriteInputTokens: number | null;
  cacheWriteTtl: Exclude<CacheWriteTtl, "provider"> | null;
  reasoningTokens: number | null;
  outputTokens: number | null;
  tokens: number | null;
  requestId: string | null;
  httpStatus: number | null;
  errorCategory: string | null;
  errorOrigin: ErrorOrigin | null;
  toolUse: ToolUseDiagnostics | null;
  routing: RoutingDiagnostics | null;
  accountId: string | null;
  candidateKind: "account" | "source";
  apiEquivalent: UsageTotals["apiEquivalent"] | null;
  requestOrigin: CodexRequestOrigin;
};

/** Retains a safe raw tier reported by an upstream response for diagnostics. */
export function normalizeObservedServiceTier(value: unknown): ObservedServiceTier | null {
  if (typeof value !== "string") return null;
  const tier = value.trim().toLowerCase();
  return /^[a-z0-9_-]{1,48}$/.test(tier) ? tier : null;
}

export function codexRequestOriginFromErrorCategory(category: string | null): CodexRequestOrigin {
  if (category === "codex_activity_summary") return "activity_summary";
  if (category === "codex_task_title") return "task_title";
  if (category === "codex_background_blocked_activity_summary") return "blocked_activity_summary";
  if (category === "codex_background_blocked_task_title") return "blocked_task_title";
  return null;
}

type UsageEvent = LocalUsage | RemoteUsage;

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

export type RemoteUsageRowLabels = UsageRowLabels & {
  accountDisplayName: (candidateLabel: string | null | undefined) => string | null | undefined;
};

function usageRowFromEvent(
  event: UsageEvent,
  time: string,
  connection: string,
  accountId: string | null,
  candidateKind: UsageRow["candidateKind"],
  requestOrigin: CodexRequestOrigin,
): UsageRow {
  return {
    id: event.id,
    attempt: event.attempt,
    time,
    success: event.success,
    model: event.resolvedModel ?? event.requestedModel,
    requestedReasoningEffort: event.requestedReasoningEffort ?? null,
    effectiveReasoningEffort: event.effectiveReasoningEffort ?? null,
    connection,
    wireApi: event.wireApi,
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
    toolUse: event.toolUse ?? null,
    routing: event.routing ?? null,
    accountId,
    candidateKind,
    apiEquivalent: event.apiEquivalent ?? null,
    requestOrigin,
  };
}

/** Builds UI rows from local telemetry without allowing local/remote fields to drift. */
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
      requestOrigin,
    );
  });
}

/** Builds UI rows from remote telemetry using the same canonical field projection. */
export function usageRowsFromRemote(events: readonly RemoteUsage[], labels: RemoteUsageRowLabels): UsageRow[] {
  return events.map((event) => {
    const requestOrigin = codexRequestOriginFromErrorCategory(event.errorCategory);
    const connection = requestOrigin
      ? labels.backgroundConnection
      : event.candidateKind === "account"
        ? labels.accountDisplayName(event.candidateLabel) ?? labels.removedAccount
        : event.candidateLabel ?? labels.unknownConnection;
    return usageRowFromEvent(
      event,
      new Date(event.createdAtMs).toISOString(),
      connection,
      null,
      event.candidateKind,
      requestOrigin,
    );
  });
}

export function totalsFromRows(rows: UsageRow[]): UsageTotals {
  return totalsFromUsageSamples(rows.map(usageTotalsSampleFromRow));
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
