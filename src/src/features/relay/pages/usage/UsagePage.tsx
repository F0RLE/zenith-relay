import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Activity, CalendarDays, CheckCircle2, ChevronLeft, ChevronRight, CreditCard, Database, Download, Gauge, RefreshCw, SlidersHorizontal, Trash2, TrendingUp } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { RemoteUsageQuery } from "../../api/types";
import { ActionMenu, ActionMenuItem, Button, Dialog, EmptyState, OptionMenu, PageHeader, Tabs, useConfirm } from "../../components/Ui";
import { sortModelIdsForLauncher } from "../../modelGroups";
import { useRelayState } from "../../state/RelayStateProvider";
import { useRelayUsageContext } from "../../state/relayStateContext";
import { formatTokenSpeed } from "../../usageSpeed";
import { AggregateView, CompactNumber, ErrorsView, RequestDetails, RequestsView } from "./UsageReportViews";
import { AccountUsageSummary } from "./AccountUsageSummary";
import { UsageMetric } from "./UsageMetric";
import { totalsFromRows, usageRowsFromLocal, usageRowsFromRemote, type UsageRow } from "./usageData";
import { formatUsageApiEquivalent } from "./usageFormatting";
import { formatCompactNumber, formatFullNumber } from "../../usageTotals";

type View = "requests" | "models" | "connections" | "errors";
type Range = "all" | "daily" | "weekly" | "monthly";
const USAGE_SUMMARY_METRICS = ["requests", "success", "tokens", "equivalent", "generationSpeed", "e2eSpeed"] as const;
type UsageSummaryMetric = typeof USAGE_SUMMARY_METRICS[number];
const USAGE_SUMMARY_LAYOUT_KEY = "relay.usageSummaryMetrics";

function loadUsageSummaryMetrics(): Record<UsageSummaryMetric, boolean> {
  const defaults = Object.fromEntries(USAGE_SUMMARY_METRICS.map((metric) => [metric, true])) as Record<UsageSummaryMetric, boolean>;
  try {
    const stored = JSON.parse(localStorage.getItem(USAGE_SUMMARY_LAYOUT_KEY) ?? "null") as Record<string, unknown> | null;
    for (const metric of USAGE_SUMMARY_METRICS) if (typeof stored?.[metric] === "boolean") defaults[metric] = stored[metric];
    if (typeof stored?.["generationSpeed"] !== "boolean" && typeof stored?.["streamSpeed"] === "boolean") defaults.generationSpeed = stored["streamSpeed"];
  } catch { }
  return defaults;
}

export function UsagePage() {
  const { t, i18n } = useTranslation();
  const { mode, runtime, loading, busy, perform, accountDisplayName } = useRelayState();
  const { revision: usageRevision, localUsagePage, loadLocalUsage, remoteUsage, remoteUsagePage, loadRemoteUsage } = useRelayUsageContext();
  const confirm = useConfirm();
  const [view, setView] = useState<View>("requests");
  const [status, setStatus] = useState("all");
  const [range, setRange] = useState<Range>("all");
  const [modelQuery, setModelQuery] = useState("");
  const [connectionQuery, setConnectionQuery] = useState("");
  const [wireApi, setWireApi] = useState("");
  const [errorQuery, setErrorQuery] = useState("");
  const [requestQuery, setRequestQuery] = useState("");
  const [page, setPage] = useState(1);
  const [selectedAccountId, setSelectedAccountId] = useState("");
  const [usageLoading, setUsageLoading] = useState(false);
  const [usageError, setUsageError] = useState(false);
  const [selected, setSelected] = useState<UsageRow | null>(null);
  const [summaryMetrics, setSummaryMetrics] = useState(loadUsageSummaryMetrics);
  const [summarySettingsOpen, setSummarySettingsOpen] = useState(false);
  const appliedUsageRevision = useRef(usageRevision);
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const remoteUsageSupported = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("usage"));
  const runtimeReady = runtime !== null;
  const requestFiltersActive = view === "requests";
  const selectedAccount = runtime?.accounts.find((account) => account.id === selectedAccountId) ?? null;
  const selectedAccountQuery = selectedAccount?.id;
  const usageQuery = useMemo<RemoteUsageQuery>(() => {
    const model = requestFiltersActive ? modelQuery.trim() : "";
    const connection = requestFiltersActive ? connectionQuery.trim() : "";
    const error = requestFiltersActive ? errorQuery.trim() : "";
    const requestId = requestFiltersActive ? requestQuery.trim() : "";
    const selectedWireApi: NonNullable<RemoteUsageQuery["wireApi"]> | undefined = requestFiltersActive && wireApi
      ? wireApi as NonNullable<RemoteUsageQuery["wireApi"]>
      : undefined;
    const success = view === "errors" ? false : requestFiltersActive && status !== "all" ? status === "success" : undefined;
    return {
      page,
      pageSize: 50,
      ...(range !== "all" ? { range } : {}),
      ...(model ? { modelQuery: model } : {}),
      ...((selectedAccountQuery ?? connection) ? { sourceOrAccountQuery: selectedAccountQuery ?? connection } : {}),
      ...(selectedWireApi !== undefined ? { wireApi: selectedWireApi } : {}),
      ...(success !== undefined ? { success } : {}),
      ...(error ? { errorCategory: error } : {}),
      ...(requestId ? { requestIdQuery: requestId } : {}),
      // Aggregate tabs opt into only the projection they render. Requests and
      // errors keep the response lightweight while retaining their page data.
      includeEvents: view === "requests" || view === "errors",
      includeModels: view === "models",
      includePoolMembers: view === "connections",
    };
  }, [page, range, modelQuery, connectionQuery, wireApi, status, errorQuery, requestQuery, view, selectedAccountQuery, requestFiltersActive]);

  useEffect(() => {
    if (mode === "zenith" || !runtimeReady || !remoteUsageSupported) {
      setUsageLoading(false);
      return;
    }
    let active = true;
    const usageChanged = appliedUsageRevision.current !== usageRevision;
    appliedUsageRevision.current = usageRevision;
    setUsageLoading(true);
    setUsageError(false);
    const load = mode === "local" ? loadLocalUsage : loadRemoteUsage;
    load(usageQuery, { force: usageChanged })
      .catch(() => active && setUsageError(true))
      .finally(() => active && setUsageLoading(false));
    return () => { active = false; };
  }, [mode, runtimeReady, usageRevision, remoteUsageSupported, usageQuery, loadLocalUsage, loadRemoteUsage]);

  useEffect(() => {
    setPage(1);
    setSelected(null);
    setSelectedAccountId("");
  }, [mode]);

  const accountLabels = useMemo(() => new Map(runtime?.accounts.map((account) => [account.id, account.label]) ?? []), [runtime?.accounts]);
  const sourceLabels = useMemo(() => new Map(runtime?.sources.map((source) => [source.id, source.name]) ?? []), [runtime?.sources]);
  const rows = useMemo<UsageRow[]>(() => {
    if (mode === "zenith") return [];
    const labels = {
      backgroundConnection: t("codex.backgroundConnection"),
      unknownAccount: t("accounts.importUnknownAccount"),
      removedAccount: t("usage.removedAccount"),
      unknownConnection: t("common.unknown"),
    };
    if (mode === "remote") {
      return usageRowsFromRemote(remoteUsage, {
        ...labels,
        accountDisplayName: (candidateLabel) => accountDisplayName(null, candidateLabel),
      });
    }
    return usageRowsFromLocal(localUsagePage?.events ?? [], {
      ...labels,
      accountLabels,
      sourceLabels,
    });
  }, [mode, remoteUsage, localUsagePage?.events, accountLabels, sourceLabels, accountDisplayName, t]);
  useEffect(() => {
    if (!selected) return;
    const current = rows.find((row) => row.id === selected.id)
      ?? (selected.requestId ? rows.find((row) => row.requestId === selected.requestId) : undefined);
    if (current !== selected) setSelected(current ?? null);
  }, [rows, selected]);
  const cutoff = useMemo(() => range === "all" ? 0 : Date.now() - (range === "daily" ? 1 : range === "weekly" ? 7 : 30) * 24 * 60 * 60 * 1_000, [range]);
  const filtered = useMemo(() => {
    if (mode !== "zenith") return rows;
    const normalizedRequestQuery = requestFiltersActive ? requestQuery.trim().toLocaleLowerCase() : "";
    const normalizedModelQuery = requestFiltersActive ? modelQuery.trim().toLocaleLowerCase() : "";
    const normalizedConnectionQuery = requestFiltersActive ? connectionQuery.trim().toLocaleLowerCase() : "";
    const normalizedErrorQuery = requestFiltersActive ? errorQuery.trim() : "";
    return rows.filter((item) => {
      if (new Date(item.time).getTime() < cutoff) return false;
      if (view === "errors") return !item.success;
      if (!requestFiltersActive) return true;
      return (status === "all" || (status === "success" ? item.success : !item.success))
        && (!normalizedRequestQuery || item.requestId?.toLocaleLowerCase().includes(normalizedRequestQuery))
        && (!normalizedModelQuery || item.model?.toLocaleLowerCase().includes(normalizedModelQuery))
        && (!normalizedConnectionQuery || item.connection.toLocaleLowerCase().includes(normalizedConnectionQuery))
        && (!wireApi || item.wireApi === wireApi)
        && (!normalizedErrorQuery || item.errorCategory === normalizedErrorQuery);
    });
  }, [connectionQuery, cutoff, errorQuery, mode, modelQuery, requestFiltersActive, requestQuery, rows, status, view, wireApi]);
  const usagePage = mode === "local" ? localUsagePage : mode === "remote" ? remoteUsagePage : null;
  const totals = usagePage?.totals ?? totalsFromRows(filtered);
  const averageGenerationSpeed = totals.generationMs && totals.generationOutputTokens ? totals.generationOutputTokens * 1_000 / totals.generationMs : null;
  const averageE2eSpeed = totals.speedDurationMs && totals.speedOutputTokens ? totals.speedOutputTokens * 1_000 / totals.speedDurationMs : null;
  const successRate = totals.requests ? Math.round(totals.successfulRequests / totals.requests * 100) : null;
  const speedUnit = t("usage.tokensPerSecondUnit");
  useEffect(() => {
    try { localStorage.setItem(USAGE_SUMMARY_LAYOUT_KEY, JSON.stringify(summaryMetrics)); } catch { }
  }, [summaryMetrics]);
  const timeFormatter = useMemo(() => new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "medium" }), [locale]);
  const formatTime = useCallback((value: string) => timeFormatter.format(new Date(value)), [timeFormatter]);
  const resetPage = (work: () => void) => { work(); setPage(1); setSelected(null); };
  const exportRows = () => perform("usage-export", () => relayCommands.exportUsage(filtered.map((row) => ({ time: row.time, success: row.success, model: row.model, requestedReasoningEffort: row.requestedReasoningEffort, effectiveReasoningEffort: row.effectiveReasoningEffort, connection: row.connection, latencyMs: row.duration, ttftMs: row.ttft, inputTokens: row.inputTokens, cachedInputTokens: row.cachedInputTokens, cacheWriteInputTokens: row.cacheWriteInputTokens, cacheWriteTtl: row.cacheWriteTtl, reasoningTokens: row.reasoningTokens, outputTokens: row.outputTokens, tokens: row.tokens, requestId: row.requestId, httpStatus: row.httpStatus, errorCategory: row.errorCategory, errorOrigin: row.errorOrigin, ...(row.serviceTier ? { serviceTier: row.serviceTier } : {}), appliedServiceTier: row.appliedServiceTier }))), "feedback.exported");
  const clearLogs = async () => {
    if (!await confirm(t("usage.clearConfirm"), { danger: true })) return;
    setPage(1);
    await perform("usage-clear", () => mode === "local" ? relayCommands.clearLocalUsage() : relayCommands.remoteAction({ type: "clear_usage" }), "feedback.cleared");
  };
  const canClear = mode === "local" || (mode === "remote" && remoteUsageSupported);
  const refreshUsage = async () => {
    setUsageLoading(true);
    setUsageError(false);
    try {
      const load = mode === "local" ? loadLocalUsage : loadRemoteUsage;
      await load(usageQuery, { force: true });
    } catch {
      setUsageError(true);
    } finally {
      setUsageLoading(false);
    }
  };
  const modelGroups = usagePage?.models;
  const poolMemberGroups = useMemo(() => usagePage?.poolMembers?.map((group) => ({
    ...group,
    label: mode === "remote"
      ? accountDisplayName(null, group.label) ?? group.label ?? t("usage.removedAccount")
      : accountLabels.get(group.key) ?? sourceLabels.get(group.key) ?? group.label ?? t("common.unknown"),
  })), [accountDisplayName, accountLabels, mode, sourceLabels, t, usagePage?.poolMembers]);
  const modelOptionIds = useMemo(() => sortModelIdsForLauncher([...new Map(
    [...(runtime?.gateway.visibleModelIds ?? []), ...(modelGroups?.map((group) => group.key) ?? []), ...rows.flatMap((row) => row.model ? [row.model] : []), ...(modelQuery ? [modelQuery] : [])]
      .filter(Boolean)
      .map((value) => [value.toLowerCase(), value] as const),
  ).values()]), [modelGroups, modelQuery, rows, runtime?.gateway.visibleModelIds]);
  const modelOptions = useMemo(() => [{ value: "", label: t("usage.anyModel") }, ...modelOptionIds.map((value) => ({ value, label: value }))], [modelOptionIds, t]);
  const poolMemberOptionSource = useMemo(() => [
    ...(poolMemberGroups ?? []),
    ...(runtime?.accounts ?? []).map((account) => ({ key: account.id, label: account.label })),
    ...(runtime?.sources ?? []).map((source) => ({ key: source.id, label: source.name })),
    ...rows.map((row) => ({ key: row.candidateKey, label: row.connection })),
  ], [poolMemberGroups, rows, runtime?.accounts, runtime?.sources]);
  const poolMemberOptions = useMemo(() => [{ value: "", label: t("usage.anyPoolMember") }, ...Array.from(new Map(poolMemberOptionSource
    .filter((group) => group.key)
    .map((group) => ({ value: group.key, label: group.label || group.key }))
    .sort((left, right) => left.label.localeCompare(right.label, i18n.language))
    .map((option) => [option.label, option] as const)).values())], [i18n.language, poolMemberOptionSource, t]);
  const errorRows = useMemo(() => filtered.filter((item) => !item.success), [filtered]);
  const clearFilters = () => {
    setStatus("all"); setModelQuery(""); setConnectionQuery("");
    setWireApi(""); setErrorQuery(""); setRequestQuery("");
    setPage(1); setSelected(null);
  };

  if (mode === "remote" && !remoteUsageSupported) {
    return <section className="relay-page"><PageHeader title={t("nav.usage")} subtitle={t("usage.subtitle")} /><EmptyState title={t("common.unsupported")} description={t("remote.capabilityUnavailable")} /></section>;
  }

  return <section className="relay-page">
    <PageHeader title={t("nav.usage")} subtitle={t("usage.subtitle")} actions={<><ActionMenu className="usage-overflow"><ActionMenuItem icon={<SlidersHorizontal aria-hidden />} onClick={() => setSummarySettingsOpen(true)}>{t("usage.configureSummary")}</ActionMenuItem><ActionMenuItem icon={<Download aria-hidden />} disabled={usageLoading || busy === "usage-export"} onClick={exportRows}>{t("common.export")}</ActionMenuItem><ActionMenuItem danger icon={<Trash2 aria-hidden />} disabled={!canClear} title={!canClear ? t("usage.clearUnavailable") : undefined} onClick={clearLogs}>{t("usage.clearLogs")}</ActionMenuItem></ActionMenu><Button variant="primary" icon={<RefreshCw aria-hidden />} busy={loading || usageLoading} onClick={() => void refreshUsage()}>{t("common.refresh")}</Button></>} />
    <div className="usage-view-toolbar">
      <Tabs value={view} onChange={(id) => { setView(id as View); setPage(1); setSelected(null); }} label={t("usage.views")} items={[{ id: "requests", label: t("usage.requests") }, { id: "models", label: t("common.models") }, { id: "connections", label: t("usage.poolMembers") }, { id: "errors", label: t("overview.errors") }]} />
      <div className="usage-scope-controls">
        {mode !== "zenith" && runtime?.accounts.length ? <OptionMenu className="usage-account-menu" label={t("usage.account")} value={selectedAccountId} onChange={(value) => resetPage(() => { setSelectedAccountId(value); setConnectionQuery(""); })} options={[{ value: "", label: t("usage.allAccounts") }, ...runtime.accounts.map((account) => ({ value: account.id, label: account.label }))]} /> : null}
        <OptionMenu className="usage-range-menu" label={t("usage.range")} value={range} onChange={(value) => resetPage(() => setRange(value as Range))} icon={<CalendarDays aria-hidden />} options={[{ value: "daily", label: t("usage.daily") }, { value: "weekly", label: t("usage.weekly") }, { value: "monthly", label: t("usage.monthly") }, { value: "all", label: t("common.all") }]} />
      </div>
    </div>
    {selectedAccount ? <AccountUsageSummary account={selectedAccount} totals={totals} /> : null}
    {USAGE_SUMMARY_METRICS.some((metric) => summaryMetrics[metric]) ? <section className="usage-overview" aria-label={t("usage.summary")}>
      {summaryMetrics.requests || summaryMetrics.success || summaryMetrics.tokens || summaryMetrics.equivalent || summaryMetrics.generationSpeed || summaryMetrics.e2eSpeed ? <div className="usage-metrics">
        {summaryMetrics.requests ? <UsageMetric icon={<Activity aria-hidden />} label={t("usage.requests")} value={<CompactNumber value={totals.requests} locale={i18n.language} />} /> : null}
        {summaryMetrics.success ? <UsageMetric icon={<CheckCircle2 aria-hidden />} label={t("common.success")} value={successRate == null ? "-" : `${successRate}%`} detail={`${formatFullNumber(totals.successfulRequests, i18n.language)} / ${formatFullNumber(totals.requests, i18n.language)}`} /> : null}
        {summaryMetrics.tokens ? <UsageMetric icon={<Database aria-hidden />} label={t("usage.totalTokens")} value={<CompactNumber value={totals.totalTokens} locale={i18n.language} />} detail={`${t("usage.inputShort")} ${formatCompactNumber(totals.inputTokens, i18n.language)} · ${t("usage.outputShort")} ${formatCompactNumber(totals.outputTokens, i18n.language)} · ${t("usage.cachedShort")} ${totals.cachedInputSamples ? formatCompactNumber(totals.cachedInputTokens, i18n.language) : "—"}${totals.cacheWriteInputSamples ? ` · ${t("usage.cacheWriteShort")} ${formatCompactNumber(totals.cacheWriteInputTokens ?? 0, i18n.language)}` : ""}`} title={t("usage.tokenCompositionHint")} /> : null}
        {summaryMetrics.equivalent ? <UsageMetric icon={<CreditCard aria-hidden />} label={t("usage.apiEquivalent")} value={formatUsageApiEquivalent(totals.apiEquivalent, i18n.language)} detail={t("usage.apiEquivalentCoverage", { priced: formatCompactNumber(totals.apiEquivalent.pricedTokens, i18n.language), unpriced: formatCompactNumber(totals.apiEquivalent.unpricedTokens, i18n.language) })} title={t("usage.apiEquivalentHint", { count: formatFullNumber(totals.apiEquivalent.unpricedTokens, i18n.language) })} /> : null}
        {summaryMetrics.generationSpeed ? <UsageMetric icon={<TrendingUp aria-hidden />} label={t("usage.generationSpeed")} value={formatTokenSpeed(averageGenerationSpeed, i18n.resolvedLanguage ?? i18n.language, speedUnit)} title={t("usage.generationSpeedHint")} /> : null}
        {summaryMetrics.e2eSpeed ? <UsageMetric icon={<Gauge aria-hidden />} label={t("usage.summaryMetrics.e2eSpeed")} value={formatTokenSpeed(averageE2eSpeed, i18n.resolvedLanguage ?? i18n.language, speedUnit)} /> : null}
      </div> : null}
    </section> : null}
    {view === "requests" ? <RequestsView rows={filtered} status={status} setStatus={(value) => resetPage(() => setStatus(value))} modelQuery={modelQuery} modelOptions={modelOptions} setModelQuery={(value) => resetPage(() => setModelQuery(value))} connectionQuery={connectionQuery} poolMemberOptions={poolMemberOptions} setConnectionQuery={(value) => resetPage(() => setConnectionQuery(value))} wireApi={wireApi} setWireApi={(value) => resetPage(() => setWireApi(value))} errorQuery={errorQuery} setErrorQuery={(value) => resetPage(() => setErrorQuery(value))} requestQuery={requestQuery} setRequestQuery={(value) => resetPage(() => setRequestQuery(value))} clearFilters={clearFilters} formatTime={formatTime} onSelect={setSelected} /> : null}
    {view === "models" ? <AggregateView rows={filtered} {...(modelGroups ? { groups: modelGroups } : {})} field="model" empty={t("usage.empty")} /> : null}
    {view === "connections" ? <AggregateView rows={filtered} {...(poolMemberGroups ? { groups: poolMemberGroups } : {})} field="connection" empty={t("usage.empty")} /> : null}
    {view === "errors" ? <ErrorsView rows={errorRows} formatTime={formatTime} onSelect={setSelected} /> : null}
    {usageError ? <p role="alert" className="form-note error-text">{t("usage.remoteLoadFailed")}</p> : null}
    {(view === "requests" || view === "errors") && usagePage && usagePage.totalPages > 1 ? <nav className="usage-pagination" aria-label={t("usage.pagination")}><Button variant="secondary" icon={<ChevronLeft aria-hidden />} disabled={page <= 1 || usageLoading} onClick={() => setPage((value) => Math.max(1, value - 1))}>{t("common.back")}</Button><span>{t("usage.page", { page: usagePage.page, total: usagePage.totalPages })}</span><Button variant="secondary" icon={<ChevronRight aria-hidden />} disabled={page >= usagePage.totalPages || usageLoading} onClick={() => setPage((value) => value + 1)}>{t("common.continue")}</Button></nav> : null}
    {selected ? <RequestDetails row={selected} onClose={() => setSelected(null)} /> : null}
    {summarySettingsOpen ? <Dialog title={t("usage.configureSummary")} onClose={() => setSummarySettingsOpen(false)} closeOnBackdrop>
      <div className="usage-summary-settings">
        {USAGE_SUMMARY_METRICS.map((metric) => <label key={metric}><input type="checkbox" checked={summaryMetrics[metric]} onChange={(event) => setSummaryMetrics((current) => ({ ...current, [metric]: event.target.checked }))} /><span>{t(`usage.summaryMetrics.${metric}`)}</span></label>)}
      </div>
    </Dialog> : null}
  </section>;
}
