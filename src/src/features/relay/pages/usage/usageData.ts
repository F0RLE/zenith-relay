import type {
  CacheWriteTtl,
  DefaultServiceTier,
  ErrorOrigin,
  ReasoningEffort,
  RoutingDiagnostics,
  ToolUseDiagnostics,
  UsageTotals,
} from "../../api/types";
import { measureTokenSpeed, type TokenSpeedSample } from "../../usageSpeed";
import { emptyUsageTotals } from "../../usageTotals";

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
  appliedServiceTier: DefaultServiceTier | null;
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

export function codexRequestOriginFromErrorCategory(category: string | null): CodexRequestOrigin {
  if (category === "codex_activity_summary") return "activity_summary";
  if (category === "codex_task_title") return "task_title";
  if (category === "codex_background_blocked_activity_summary") return "blocked_activity_summary";
  if (category === "codex_background_blocked_task_title") return "blocked_task_title";
  return null;
}

export function totalsFromRows(rows: UsageRow[]): UsageTotals {
  return rows.reduce<UsageTotals>((totals, row) => {
    const outputTokens = row.success ? Math.max(0, row.outputTokens ?? 0) : 0;
    totals.requests += 1;
    totals.successfulRequests += Number(row.success);
    totals.latencyMs += row.duration;
    if (row.ttft != null) {
      totals.ttftMs += row.ttft;
      totals.ttftSamples += 1;
    }
    const generation = measureTokenSpeed(usageSpeedSample(row));
    if (generation) {
      totals.generationMs += generation.durationMs;
      totals.generationSamples += 1;
      totals.generationOutputTokens += generation.outputTokens;
    }
    totals.inputTokens += row.inputTokens ?? 0;
    if (row.cachedInputTokens != null) {
      totals.cachedInputTokens += row.cachedInputTokens;
      totals.cachedInputSamples += 1;
    }
    if (row.cacheWriteInputTokens != null) {
      totals.cacheWriteInputTokens = (totals.cacheWriteInputTokens ?? 0) + row.cacheWriteInputTokens;
      totals.cacheWriteInputSamples = (totals.cacheWriteInputSamples ?? 0) + 1;
    }
    totals.reasoningTokens += row.reasoningTokens ?? 0;
    totals.outputTokens += row.outputTokens ?? 0;
    totals.totalTokens += row.tokens ?? 0;
    if (outputTokens > 0 && row.duration > 0) {
      totals.speedOutputTokens += outputTokens;
      totals.speedDurationMs += row.duration;
    }
    if (row.apiEquivalent) {
      totals.apiEquivalent.microUsd += row.apiEquivalent.microUsd;
      totals.apiEquivalent.pricedTokens += row.apiEquivalent.pricedTokens;
      totals.apiEquivalent.unpricedTokens += row.apiEquivalent.unpricedTokens;
    } else {
      totals.apiEquivalent.unpricedTokens += row.tokens ?? 0;
    }
    return totals;
  }, emptyUsageTotals());
}

export function usageSpeedSample(row: UsageRow): TokenSpeedSample {
  return {
    success: row.success,
    outputTokens: row.outputTokens,
    reasoningTokens: row.reasoningTokens,
    durationMs: row.generationMs,
  };
}
