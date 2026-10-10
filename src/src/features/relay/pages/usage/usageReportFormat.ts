import type { TFunction } from "i18next";
import type { ErrorOrigin, ReasoningEffort, ToolUseDiagnostics, UsageTotals } from "../../api/types";
import { formatNumber } from "../../numberFormatting";
import { observedTokensPerSecond } from "../../usageSpeed";
import { cacheLifetime } from "./cacheLifetime";
import { normalizeObservedServiceTier, totalsFromRows } from "./usageData";
import type { CodexRequestOrigin, UsageRow } from "./usageData";

export function formatTiming(ttft: number | null, duration: number, locale: string, t: TFunction) {
  return `${formatDurationMs(ttft, locale, t)} / ${formatDurationMs(duration, locale, t)}`;
}

export type AggregateRow = {
  name: string;
  requests: number;
  success: number;
  inputTokens: number;
  cachedInputTokens: number;
  cachedInputSamples: number;
  cacheWriteInputTokens: number;
  cacheWriteInputSamples: number;
  reasoningTokens: number;
  outputTokens: number;
  tokens: number;
  ttft: number;
  ttftCount: number;
  duration: number;
  generationSpeed: number | null;
  apiEquivalent: UsageTotals["apiEquivalent"];
};

export function formatDurationMs(elapsedMs: number | null, locale: string, t: TFunction): string {
  if (elapsedMs == null || !Number.isFinite(elapsedMs)) return "—";
  if (elapsedMs >= 1000) return t("usage.durationSeconds", { value: formatNumber(elapsedMs / 1000, locale, { maximumFractionDigits: 1 }) });
  return t("usage.durationMilliseconds", { value: Math.round(elapsedMs) });
}

export function requestStatusLabel(row: Pick<UsageRow, "success" | "requestOrigin">, t: TFunction): string {
  if (row.requestOrigin?.startsWith("blocked_")) return t("codex.backgroundBlocked");
  if (row.requestOrigin) return t("common.success");
  return row.success ? t("common.success") : t("common.failed");
}

export function formatRequestOrigin(origin: Exclude<CodexRequestOrigin, null>, t: TFunction): string {
  if (origin === "activity_summary" || origin === "blocked_activity_summary") return t("codex.activitySummary");
  return t("codex.taskTitle");
}

export function formatReasoningEffort(effort: ReasoningEffort | null, t: TFunction): string {
  return effort ? t(`usage.reasoningEfforts.${effort}`) : "-";
}

export function formatReasoningSummary(row: Pick<UsageRow, "requestedReasoningEffort" | "effectiveReasoningEffort">, t: TFunction): string {
  if (row.requestedReasoningEffort && row.effectiveReasoningEffort && row.requestedReasoningEffort !== row.effectiveReasoningEffort) {
    return t("usage.reasoningEffortChanged", { requested: formatReasoningEffort(row.requestedReasoningEffort, t), effective: formatReasoningEffort(row.effectiveReasoningEffort, t) });
  }
  return formatReasoningEffort(row.effectiveReasoningEffort ?? row.requestedReasoningEffort, t);
}

export function formatServiceTier(row: Pick<UsageRow, "serviceTier" | "appliedServiceTier">, t: TFunction, fallback = "—") {
  return row.serviceTier ? t(`pool.serviceTiers.${row.serviceTier}`) : fallback;
}

export function formatObservedServiceTier(row: Pick<UsageRow, "appliedServiceTier">): string | null {
  return normalizeObservedServiceTier(row.appliedServiceTier);
}

export function formatWireApi(wireApi: string | null, t: TFunction): string {
  if (wireApi === "responses") return t("usage.protocols.responses");
  if (wireApi === "messages") return t("usage.protocols.messages");
  if (wireApi === "chat_completions") return t("usage.protocols.chatCompletions");
  if (wireApi === "gemini") return t("usage.protocols.gemini");
  return wireApi ?? "—";
}

export function formatTransport(transportKind: string | null | undefined, t: TFunction): string {
  if (transportKind === "websocket") return t("usage.transports.websocket");
  if (transportKind === "http") return t("usage.transports.http");
  return transportKind ?? "—";
}

export function formatEndpointKind(value: string | null | undefined, wireApi: string | null, t: TFunction): string {
  if (value === "excel_basis_points") return t("usage.endpoints.excelBasisPoints");
  if (value === "responses") return t("usage.protocols.responses");
  if (value === "chat_completions") return t("usage.protocols.chatCompletions");
  if (value === "messages") return t("usage.protocols.messages");
  if (value === "gemini") return t("usage.protocols.gemini");
  return value ?? formatWireApi(wireApi, t);
}

export function formatErrorCategory(category: string | null, t: TFunction): string {
  if (!category) return t("common.unknown");
  return t(`usage.errorCategories.${category}`, { defaultValue: category.replace(/_/g, " ") });
}

export function formatErrorOrigin(origin: ErrorOrigin | null, t: TFunction): string {
  return origin ? t(`usage.errorOrigins.${origin}`) : t("common.unknown");
}

export function prefixErrorOrigin(origin: ErrorOrigin | null | undefined, message: string): string {
  if (!origin) return message;
  const label = origin === "account" ? "Account" : origin === "provider" ? "Provider" : "Relay";
  const unprefixed = message.replace(/^(?:Account|Provider|Relay):\s*/i, "");
  return `${label}: ${unprefixed}`;
}

export function formatToolChoice(choice: ToolUseDiagnostics["toolChoice"], t: TFunction): string {
  return t(`usage.toolChoices.${choice}`);
}

export function formatTerminalOutput(terminalOutput: ToolUseDiagnostics["terminalOutput"], t: TFunction): string {
  return t(`usage.terminalOutputs.${terminalOutput}`);
}

export function aggregateRowsFromUsage(rows: UsageRow[], field: "model" | "connection", unknown: string): AggregateRow[] {
  const groups = new Map<string, UsageRow[]>();
  for (const row of rows) {
    const key = row[field] || unknown;
    const group = groups.get(key);
    if (group) group.push(row);
    else groups.set(key, [row]);
  }
  return [...groups.entries()].map(([groupLabel, groupRows]) => aggregateRowFromTotals(groupLabel, totalsFromRows(groupRows)));
}

export function aggregateRowFromTotals(rowLabel: string, totals: UsageTotals): AggregateRow {
  return {
    name: rowLabel,
    requests: totals.requests,
    success: totals.successfulRequests,
    inputTokens: totals.inputTokens,
    cachedInputTokens: totals.cachedInputTokens,
    cachedInputSamples: totals.cachedInputSamples,
    cacheWriteInputTokens: totals.cacheWriteInputTokens ?? 0,
    cacheWriteInputSamples: totals.cacheWriteInputSamples ?? 0,
    reasoningTokens: totals.reasoningTokens,
    outputTokens: totals.outputTokens,
    tokens: totals.totalTokens,
    ttft: totals.ttftMs,
    ttftCount: totals.ttftSamples,
    duration: totals.latencyMs,
    generationSpeed: observedTokensPerSecond(totals.generationOutputTokens, totals.generationMs),
    apiEquivalent: totals.apiEquivalent,
  };
}

export type CacheTouch = {
  model: string | null;
  cacheWriteTtl: string | null;
  touchedAt: string;
};

export function cacheRemainingLabel(life: ReturnType<typeof cacheLifetime>, translate: TFunction) {
  if (life.expiry === "unknown") return translate("usage.cacheReport.unknown");
  if (life.expiry === "minimum_elapsed") return translate("usage.cacheReport.minimumElapsed");
  if (life.expiry === "elapsed") return translate("usage.cacheReport.elapsed");
  const remaining = life.remainingMs ?? 0;
  if (remaining < 60_000) return translate("usage.cacheReport.soon");
  return translate("usage.cacheReport.open", { count: Math.max(1, Math.round(remaining / 60_000)) });
}
