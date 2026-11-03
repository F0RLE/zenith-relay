import { defaultLocale, translate } from "../../i18n/catalog";
import type { Locale } from "../../i18n/catalog";

export type Execution = "completed" | "running" | "failed";
export type Accounting = "posted" | "pending" | "held";
export type ComponentId = "input" | "cacheRead" | "cacheWrite" | "output";

export interface TokenCounters {
  input: number | null;
  output: number | null;
  cacheRead: number | null;
  cacheWrite: number | null;
  reasoning: number | null;
}

// Customer-facing view model for the web preview.
export interface RequestRecord {
  id: string;
  startedAt: string;
  model: { id: string; name: string; reasoningEffort: string };
  key: { id: string; name: string; mask: string };
  execution: Execution;
  accounting: Accounting;
  protocol: string;
  transport: string;
  tier: string;
  tokens: TokenCounters;
  timing: { firstTokenMs: number | null; totalMs: number | null; generationMs: number | null };
  billing: {
    revision: string;
    rates: Record<ComponentId, bigint>;
    components: Record<ComponentId, bigint | null>;
    postedMicroUsd: bigint | null;
    reservedMicroUsd: bigint | null;
  };
  client: { label: string; userAgent: string };
  response: { httpStatus: number | null; result: string; errorCode: string | null; guidance: string | null };
  context: { bodyBytes: number; instructionBytes: number; conversationBytes: number; toolSchemaBytes: number } | null;
  tools: { declared: number; forwarded: number; calls: number; choice: string; result: string } | null;
}

export const executionLabels = { completed: "status.completed", running: "status.running", failed: "status.error" } as const;
export const accountingLabels = { posted: "status.posted", pending: "status.pending", held: "status.held" } as const;

export function tokenTotal(tokens: TokenCounters): number | null {
  return tokens.input === null || tokens.output === null ? null : tokens.input + tokens.output;
}

export function componentTokens(tokens: TokenCounters): Record<ComponentId, number | null> {
  return {
    input: tokens.input === null || tokens.cacheRead === null || tokens.cacheWrite === null
      ? null : tokens.input - tokens.cacheRead - tokens.cacheWrite,
    cacheRead: tokens.cacheRead,
    cacheWrite: tokens.cacheWrite,
    output: tokens.output,
  };
}

export function componentTotal(components: Record<ComponentId, bigint | null>): bigint | null {
  const values = Object.values(components);
  return values.some(value => value === null) ? null : values.reduce<bigint>((sum, value) => sum + value!, 0n);
}

export function formatInteger(value: number | null, locale: Locale = defaultLocale): string {
  return value === null ? "—" : value.toLocaleString(locale);
}

export function formatMoney(value: bigint | null, digits = 6, locale: Locale = defaultLocale): string {
  if (value === null) return "—";
  const negative = value < 0n;
  const magnitude = negative ? -value : value;
  const divisor = 10n ** BigInt(6 - digits);
  const rounded = (magnitude + divisor / 2n) / divisor;
  const base = 10n ** BigInt(digits);
  const decimal = new Intl.NumberFormat(locale).formatToParts(1.1).find(part => part.type === "decimal")!.value;
  return `${negative ? "−" : ""}${(rounded / base).toLocaleString(locale)}${digits ? decimal + (rounded % base).toString().padStart(digits, "0") : ""}`;
}

export function formatTime(iso: string, locale: Locale = defaultLocale): string {
  return new Date(iso).toLocaleString(locale, { timeZone: "UTC", dateStyle: "short", timeStyle: "medium" });
}

export function formatDuration(ms: number | null, locale: Locale = defaultLocale): string {
  return ms === null ? "—" : new Intl.NumberFormat(locale, { style: "unit", unit: "second", unitDisplay: "short", maximumFractionDigits: 2 }).format(ms / 1000);
}

export function generationSpeed(record: RequestRecord): number | null {
  return record.tokens.output === null || !record.timing.generationMs
    ? null : record.tokens.output / record.timing.generationMs * 1000;
}

export function usageTotals(rows: readonly RequestRecord[]) {
  const known = rows.filter(row => tokenTotal(row.tokens) !== null);
  const finished = rows.filter(row => row.execution !== "running");
  const measured = rows.filter(row => generationSpeed(row) !== null);
  const posted = rows.filter(row => row.accounting === "posted");
  const output = measured.reduce((sum, row) => sum + row.tokens.output!, 0);
  const generationMs = measured.reduce((sum, row) => sum + row.timing.generationMs!, 0);
  const totalMs = measured.reduce((sum, row) => sum + row.timing.totalMs!, 0);
  return {
    count: rows.length, known: known.length, tokens: known.reduce((sum, row) => sum + tokenTotal(row.tokens)!, 0),
    success: rows.filter(row => row.execution === "completed").length, finished: finished.length,
    charge: posted.some(row => row.billing.postedMicroUsd === null) ? null
      : posted.reduce((sum, row) => sum + row.billing.postedMicroUsd!, 0n),
    pending: rows.filter(row => row.accounting !== "posted").length,
    generation: generationMs ? output / generationMs * 1000 : null, e2e: totalMs ? output / totalMs * 1000 : null,
  };
}

export function formatSpeed(value: number | null, locale: Locale = defaultLocale): string {
  return value === null ? "—" : translate(locale, "usage.tokensPerSecond", { value: value.toLocaleString(locale, { minimumFractionDigits: 1, maximumFractionDigits: 1 }) });
}

export function supportSummary(record: RequestRecord, locale: Locale = defaultLocale): string {
  const t = (key: Parameters<typeof translate>[1]) => translate(locale, key);
  return [
    `ID: ${record.id}`, `${t("usage.started")}: ${formatTime(record.startedAt, locale)} UTC`,
    `${t("common.model")}: ${record.model.id}`, `${t("common.apiKey")}: ${record.key.name} · ${record.key.mask}`,
    `${t("usage.execution")}: ${t(executionLabels[record.execution])}`, `${t("usage.accounting")}: ${t(accountingLabels[record.accounting])}`,
    `${t("usage.protocolTransport")}: ${record.protocol} / ${record.transport}`,
    `${t("common.tokens")}: ${formatInteger(tokenTotal(record.tokens), locale)}`,
    `${t("common.charged")}: ${formatMoney(record.billing.postedMicroUsd, 6, locale)} USD`,
    ...(record.response.errorCode ? [`${t("common.errorCode")}: ${record.response.errorCode}`] : []),
  ].join("\n");
}
