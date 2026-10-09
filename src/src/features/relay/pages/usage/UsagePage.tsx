import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Activity, CalendarDays, CheckCircle2, CreditCard, Database, Download, Gauge, RefreshCw, SlidersHorizontal, Trash2, TrendingUp, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { RemoteUsageQuery, UsageTotals } from "../../api/types";
import { ActionMenu, ActionMenuItem, EmptyState, IconButton, OptionMenu, PageHeader, Tabs, useConfirm } from "../../components/Ui";
import { orderModelIdsBySnapshot } from "../../modelGroups";
import { useRelayState } from "../../state/RelayStateProvider";
import { useRelayUsageContext } from "../../state/relayStateContext";
import { formatTokenSpeed, observedTokensPerSecond } from "../../usageSpeed";
import { AggregateView } from "./AggregateView";
import { ErrorsView } from "./ErrorsView";
import { RequestDetails } from "./RequestDetails";
import { RequestFilters } from "./RequestFilters";
import { RequestsView } from "./RequestsView";
import { CompactNumber } from "./usageReportParts";
import { UsageMetric } from "./UsageMetric";
import { UsagePagination } from "./UsagePagination";
import { AccountUsageSummary } from "./AccountUsageSummary";
import { totalsFromRows, usageRowsFromLocal, usageRowsFromRemote, type UsageRow } from "./usageData";
import { formatUsageApiEquivalent } from "./usageFormatting";
import { formatCompactNumber, formatFullNumber } from "../../usageTotals";

type View = "requests" | "models" | "connections" | "errors";
type Range = "all" | "daily" | "weekly" | "monthly";
const USAGE_SUMMARY_METRICS = ["requests", "success", "tokens", "equivalent", "generationSpeed", "e2eSpeed"] as const;
type UsageSummaryMetric = typeof USAGE_SUMMARY_METRICS[number];
const REQUEST_SUMMARY_METRICS: UsageSummaryMetric[] = [...USAGE_SUMMARY_METRICS];
const AGGREGATE_SUMMARY_METRICS: UsageSummaryMetric[] = ["requests", "tokens", "equivalent"];

export function UsagePage() {
  const { t, i18n } = useTranslation();
  const { mode, runtime, loading, busy, perform, accountDisplayName } = useRelayState();
  const { usageRevision, localUsagePage, loadLocalUsage, remoteUsage, remoteUsagePage, loadRemoteUsage } = useRelayUsageContext();
  const confirm = useConfirm();
  const [view, setView] = useState<View>("requests");
  const [status, setStatus] = useState("all");
  const [range, setRange] = useState<Range>("all");
  const [modelQuery, setModelQuery] = useState("");
  const [connectionQuery, setConnectionQuery] = useState("");
  const [wireApi, setWireApi] = useState("");
  const [transport, setTransport] = useState("");
  const [errorQuery, setErrorQuery] = useState("");
  const [requestQuery, setRequestQuery] = useState("");
  const [showMoreFilters, setShowMoreFilters] = useState(false);
  const [page, setPage] = useState(1);
  const [usageLoading, setUsageLoading] = useState(false);
  const [usageError, setUsageError] = useState(false);
  const [selectedRequest, setSelectedRequest] = useState<UsageRow | null>(null);
  const appliedUsageRevision = useRef(usageRevision);
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const remoteUsageSupported = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("usage"));
  const runtimeReady = runtime !== null;
  const requestFiltersActive = view === "requests";
  const usageQuery = useMemo<RemoteUsageQuery>(() => {
    const model = modelQuery.trim();
    const connection = connectionQuery.trim();
    const error = requestFiltersActive ? errorQuery.trim() : "";
    const requestId = requestFiltersActive ? requestQuery.trim() : "";
    const selectedWireApi: NonNullable<RemoteUsageQuery["wireApi"]> | undefined = requestFiltersActive && wireApi
      ? wireApi as NonNullable<RemoteUsageQuery["wireApi"]>
      : undefined;
    const selectedTransport: NonNullable<RemoteUsageQuery["transport"]> | undefined = requestFiltersActive && transport
      ? transport as NonNullable<RemoteUsageQuery["transport"]>
      : undefined;
    const success = view === "errors" ? false : status !== "all" ? status === "success" : undefined;
    return {
      page,
      pageSize: 50,
      ...(range !== "all" ? { range } : {}),
      ...(model ? { modelQuery: model } : {}),
      ...(connection ? { sourceOrAccountQuery: connection } : {}),
      ...(selectedWireApi !== undefined ? { wireApi: selectedWireApi } : {}),
      ...(selectedTransport !== undefined ? { transport: selectedTransport } : {}),
      ...(success !== undefined ? { success } : {}),
      ...(error ? { errorCategory: error } : {}),
      ...(requestId ? { requestIdQuery: requestId } : {}),
      // Aggregate tabs opt into only the projection they render. Requests and
      // errors keep the response lightweight while retaining their page data.
      includeEvents: view === "requests" || view === "errors",
      includeModels: view === "models",
      includePoolMembers: view === "connections",
    };
  }, [page, range, modelQuery, connectionQuery, wireApi, transport, status, errorQuery, requestQuery, view, requestFiltersActive]);

  useEffect(() => {
    if (mode === "zenith" || !runtimeReady || !remoteUsageSupported) {
      setUsageLoading(false);
      return;
    }
    let isActive = true;
    const usageChanged = appliedUsageRevision.current !== usageRevision;
    appliedUsageRevision.current = usageRevision;
    setUsageLoading(true);
    setUsageError(false);
    const loadUsage = mode === "local" ? loadLocalUsage : loadRemoteUsage;
    loadUsage(usageQuery, { force: usageChanged })
      .catch(() => isActive && setUsageError(true))
      .finally(() => isActive && setUsageLoading(false));
    return () => { isActive = false; };
  }, [mode, runtimeReady, usageRevision, remoteUsageSupported, usageQuery, loadLocalUsage, loadRemoteUsage]);


  useEffect(() => {
    setPage(1);
    setSelectedRequest(null);
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
    if (!selectedRequest) return;
    const currentRow = rows.find((row) => row.id === selectedRequest.id)
      ?? (selectedRequest.requestId ? rows.find((row) => row.requestId === selectedRequest.requestId) : undefined);
    if (currentRow !== selectedRequest) setSelectedRequest(currentRow ?? null);
  }, [rows, selectedRequest]);
  const usagePage = mode === "local" ? localUsagePage : mode === "remote" ? remoteUsagePage : null;
  const totals = usagePage?.totals ?? totalsFromRows(rows);
  const averageGenerationSpeed = observedTokensPerSecond(totals.generationOutputTokens, totals.generationMs);
  const averageE2eSpeed = observedTokensPerSecond(totals.speedOutputTokens, totals.speedDurationMs);
  const successRate = totals.requests ? Math.round(totals.successfulRequests / totals.requests * 100) : null;
  const timeFormatter = useMemo(() => new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "medium" }), [locale]);
  const formatTime = useCallback((timestamp: string) => timeFormatter.format(new Date(timestamp)), [timeFormatter]);
  const resetPage = (work: () => void) => { work(); setPage(1); setSelectedRequest(null); };
  const changePage = (nextPage: number) => { setPage(nextPage); setSelectedRequest(null); };
  const exportRows = () => perform("usage-export", () => relayCommands.exportUsage(rows.map((row) => ({
    time: row.time,
    success: row.success,
    model: row.model,
    requestedReasoningEffort: row.requestedReasoningEffort,
    effectiveReasoningEffort: row.effectiveReasoningEffort,
    connection: row.connection,
    transport: row.transport,
    latencyMs: row.duration,
    ttftMs: row.ttft,
    inputTokens: row.inputTokens,
    cachedInputTokens: row.cachedInputTokens,
    cacheWriteInputTokens: row.cacheWriteInputTokens,
    cacheWriteTtl: row.cacheWriteTtl,
    reasoningTokens: row.reasoningTokens,
    outputTokens: row.outputTokens,
    tokens: row.tokens,
    requestId: row.requestId,
    httpStatus: row.httpStatus,
    errorCategory: row.errorCategory,
    errorOrigin: row.errorOrigin,
    ...(row.serviceTier ? { serviceTier: row.serviceTier } : {}),
    appliedServiceTier: row.appliedServiceTier,
  }))), "feedback.exported", { backgroundRefresh: true });
  const clearLogs = async () => {
    if (!await confirm(t("usage.clearConfirm"), { danger: true })) return;
    setPage(1);
    await perform("usage-clear", () => mode === "local" ? relayCommands.clearLocalUsage() : relayCommands.remoteAction({ type: "clear_usage" }), "feedback.cleared", { backgroundRefresh: true });
  };
  const canClear = mode === "local" || (mode === "remote" && remoteUsageSupported);
  const refreshUsage = async () => {
    setUsageLoading(true);
    setUsageError(false);
    try {
      const loadUsage = mode === "local" ? loadLocalUsage : loadRemoteUsage;
      await loadUsage(usageQuery, { force: true });
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
  const modelOptionIds = useMemo(() => orderModelIdsBySnapshot(
    [
      ...(runtime?.gateway.visibleModelIds ?? []),
      ...(modelGroups?.map((group) => group.key) ?? []),
      ...rows.flatMap((row) => row.model ? [row.model] : []),
      ...(modelQuery ? [modelQuery] : []),
    ],
    runtime?.gateway.models ?? [],
  ), [modelGroups, modelQuery, rows, runtime?.gateway.models, runtime?.gateway.visibleModelIds]);
  const modelOptions = useMemo(() => [{ value: "", label: t("usage.anyModel") }, ...modelOptionIds.map((modelId) => ({ value: modelId, label: modelId }))], [modelOptionIds, t]);
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
    .map((option) => [option.value, option] as const)).values())], [i18n.language, poolMemberOptionSource, t]);
  const selectedAccount = mode !== "zenith"
    ? runtime?.accounts.find((account) => account.id === connectionQuery)
    : undefined;
  const errorRows = useMemo(() => rows.filter((usageRow) => !usageRow.success), [rows]);
  const clearFilters = () => {
    setRange("all"); setStatus("all"); setModelQuery(""); setConnectionQuery("");
    setWireApi(""); setTransport(""); setErrorQuery(""); setRequestQuery("");
    setPage(1); setSelectedRequest(null);
  };
  const visibleSummaryMetrics = view === "requests" ? REQUEST_SUMMARY_METRICS : AGGREGATE_SUMMARY_METRICS;
  const showStatusFilter = view !== "errors";
  const showModelFilter = view !== "models";
  const showPoolMemberFilter = view !== "connections";
  const additionalFilterCount = [wireApi, transport, errorQuery, requestQuery.trim()].filter(Boolean).length;
  const scopeMenuProps = { showSelectionIndicator: false, fitContent: true };
  const scopeFilterCount = 1 + Number(showStatusFilter) + Number(showModelFilter) + Number(showPoolMemberFilter);
  const hasFilters = range !== "all"
    || (showStatusFilter && status !== "all")
    || (showModelFilter && Boolean(modelQuery))
    || (showPoolMemberFilter && Boolean(connectionQuery))
    || (requestFiltersActive && Boolean(wireApi || transport || errorQuery || requestQuery));
  const changeView = (nextView: View) => {
    setView(nextView);
    setPage(1);
    setSelectedRequest(null);
    setShowMoreFilters(false);
    if (nextView === "errors") setStatus("all");
    if (nextView === "models") setModelQuery("");
    if (nextView === "connections") setConnectionQuery("");
  };

  if (mode === "remote" && !remoteUsageSupported) {
    return <section className="relay-page relay-workspace-page"><PageHeader workspace title={t("nav.usage")} /><EmptyState title={t("common.unsupported")} description={t("remote.capabilityUnavailable")} /></section>;
  }

  return <section className="relay-page relay-workspace-page usage-page">
    <PageHeader
      title={t("nav.usage")}
      navigation={<Tabs
        value={view}
        onChange={(nextView) => changeView(nextView as View)}
        label={t("usage.views")}
        items={[
          { id: "requests", label: t("usage.requests") },
          { id: "models", label: t("common.models") },
          { id: "connections", label: t("usage.poolMembers") },
          { id: "errors", label: t("overview.errors") },
        ]}
      />}
      actions={<>
        <IconButton label={t("common.refresh")} icon={<RefreshCw aria-hidden />} busy={loading || usageLoading} onClick={() => void refreshUsage()} />
        <ActionMenu className="usage-overflow">
          <ActionMenuItem icon={<Download aria-hidden />} disabled={usageLoading || busy === "usage-export"} onClick={exportRows}>{t("common.export")}</ActionMenuItem>
          <ActionMenuItem danger icon={<Trash2 aria-hidden />} disabled={!canClear} title={!canClear ? t("usage.clearUnavailable") : undefined} onClick={clearLogs}>{t("usage.clearLogs")}</ActionMenuItem>
        </ActionMenu>
      </>}
    />
    <div className="usage-view-toolbar">
      <div className="usage-scope-controls" data-filter-count={scopeFilterCount}>
        <OptionMenu
          {...scopeMenuProps}
          className="usage-range-menu"
          label={t("usage.range")}
          value={range}
          onChange={(rangeValue) => resetPage(() => setRange(rangeValue as Range))}
          icon={<CalendarDays aria-hidden />}
          options={[
            { value: "daily", label: t("usage.daily") },
            { value: "weekly", label: t("usage.weekly") },
            { value: "monthly", label: t("usage.monthly") },
            { value: "all", label: t("common.all") },
          ]}
        />
        {showStatusFilter ? <OptionMenu
          {...scopeMenuProps}
          className="usage-status-menu"
          label={t("common.status")}
          value={status}
          onChange={(statusValue) => resetPage(() => setStatus(statusValue))}
          options={[
            { value: "all", label: t("usage.anyStatus") },
            { value: "success", label: t("common.success") },
            { value: "failed", label: t("common.failed") },
          ]}
        /> : null}
        {showModelFilter ? <OptionMenu
          {...scopeMenuProps}
          className="usage-model-menu"
          label={t("common.model")}
          value={modelQuery}
          onChange={(modelValue) => resetPage(() => setModelQuery(modelValue))}
          options={modelOptions}
        /> : null}
        {showPoolMemberFilter ? <div className="usage-member-controls"><OptionMenu
          {...scopeMenuProps}
          className="usage-pool-member-menu"
          label={t("usage.poolMember")}
          value={connectionQuery}
          onChange={(memberValue) => resetPage(() => setConnectionQuery(memberValue))}
          options={poolMemberOptions}
        />
          {requestFiltersActive ? <span className="usage-filter-toggle-wrap">
            <IconButton
              className="usage-filter-toggle"
              label={t("usage.moreFilters")}
              icon={<SlidersHorizontal aria-hidden />}
              aria-expanded={showMoreFilters}
              aria-controls="usage-request-filters"
              onClick={() => setShowMoreFilters((isVisible) => !isVisible)}
            />
            {additionalFilterCount ? <small>{additionalFilterCount}</small> : null}
          </span> : null}
        </div> : null}
        {hasFilters ? <IconButton className="usage-clear-filters" label={t("usage.clearFilters")} icon={<X aria-hidden />} onClick={clearFilters} /> : null}
      </div>
    </div>
    {requestFiltersActive && showMoreFilters ? <RequestFilters
      rows={rows}
      wireApi={wireApi}
      onWireApiChange={(value) => resetPage(() => setWireApi(value))}
      transport={transport}
      onTransportChange={(value) => resetPage(() => setTransport(value))}
      errorQuery={errorQuery}
      onErrorChange={(value) => resetPage(() => setErrorQuery(value))}
      requestQuery={requestQuery}
      onRequestChange={(value) => resetPage(() => setRequestQuery(value))}
      onReset={() => resetPage(() => {
        setWireApi(""); setTransport(""); setErrorQuery(""); setRequestQuery("");
      })}
      onClose={() => setShowMoreFilters(false)}
    /> : null}
    {visibleSummaryMetrics.length ? (
      <UsageSummary
        visibleMetrics={visibleSummaryMetrics}
        totals={totals}
        successRate={successRate}
        generationSpeed={averageGenerationSpeed}
        e2eSpeed={averageE2eSpeed}
        language={i18n.language}
        locale={i18n.resolvedLanguage ?? i18n.language}
      />
    ) : null}
    {selectedAccount && view !== "errors" ? <AccountUsageSummary account={selectedAccount} totals={totals} /> : null}
    {view === "requests" ? <RequestsView
      rows={rows}
      formatTime={formatTime}
      onSelect={setSelectedRequest}
    /> : null}
    {view === "models" ? <AggregateView rows={rows} {...(modelGroups ? { groups: modelGroups } : {})} field="model" empty={t("usage.empty")} /> : null}
    {view === "connections" ? <AggregateView rows={rows} {...(poolMemberGroups ? { groups: poolMemberGroups } : {})} field="connection" empty={t("usage.empty")} /> : null}
    {view === "errors" ? <ErrorsView rows={errorRows} formatTime={formatTime} onSelect={setSelectedRequest} /> : null}
    {usageError ? <p role="alert" className="form-note error-text">{t("usage.remoteLoadFailed")}</p> : null}
    {(view === "requests" || view === "errors") && usagePage && usagePage.page === page && usagePage.totalPages > 1 ? (
      <UsagePagination
        key={JSON.stringify([mode, view, status, range, modelQuery, connectionQuery, wireApi, transport, errorQuery, requestQuery])}
        page={page}
        totalPages={usagePage.totalPages}
        loading={usageLoading}
        onPageChange={changePage}
      />
    ) : null}
    {selectedRequest ? <RequestDetails row={selectedRequest} local={mode === "local"} onClose={() => setSelectedRequest(null)} /> : null}
  </section>;
}

function UsageSummary({
  visibleMetrics,
  totals,
  successRate,
  generationSpeed,
  e2eSpeed,
  language,
  locale,
}: {
  visibleMetrics: UsageSummaryMetric[];
  totals: UsageTotals;
  successRate: number | null;
  generationSpeed: number | null;
  e2eSpeed: number | null;
  language: string;
  locale: string;
}) {
  const { t } = useTranslation();
  const speedUnit = t("usage.tokensPerSecondUnit");
  return (
    <section className="usage-overview" aria-label={t("usage.summary")}>
      <div className="usage-metrics">
        {visibleMetrics.includes("requests") ? <UsageMetric icon={<Activity aria-hidden />} label={t("usage.requests")} value={<CompactNumber value={totals.requests} locale={language} />} /> : null}
        {visibleMetrics.includes("success") ? (
          <UsageMetric
            icon={<CheckCircle2 aria-hidden />}
            label={t("common.success")}
            value={successRate == null ? "-" : `${successRate}%`}
            detail={`${formatFullNumber(totals.successfulRequests, language)} / ${formatFullNumber(totals.requests, language)}`}
          />
        ) : null}
        {visibleMetrics.includes("tokens") ? (
          <UsageMetric
            icon={<Database aria-hidden />}
            label={t("usage.totalTokens")}
            value={<CompactNumber value={totals.totalTokens} locale={language} />}
            detail={[
              `${t("usage.inputShort")} ${formatCompactNumber(totals.inputTokens, language)}`,
              `${t("usage.outputShort")} ${formatCompactNumber(totals.outputTokens, language)}`,
              `${t("usage.cachedShort")} ${totals.cachedInputSamples ? formatCompactNumber(totals.cachedInputTokens, language) : "—"}`,
              totals.cacheWriteInputSamples
                ? `${t("usage.cacheWriteShort")} ${formatCompactNumber(totals.cacheWriteInputTokens ?? 0, language)}`
                : null,
            ].filter(Boolean).join(" · ")}
            title={t("usage.tokenCompositionHint")}
          />
        ) : null}
        {visibleMetrics.includes("equivalent") ? (
          <UsageMetric
            icon={<CreditCard aria-hidden />}
            label={t("usage.apiEquivalent")}
            value={formatUsageApiEquivalent(totals.apiEquivalent, language)}
            detail={t("usage.apiEquivalentCoverage", {
              priced: formatCompactNumber(totals.apiEquivalent.pricedTokens, language),
              unpriced: formatCompactNumber(totals.apiEquivalent.unpricedTokens, language),
            })}
            title={t("usage.apiEquivalentHint", { count: formatFullNumber(totals.apiEquivalent.unpricedTokens, language) })}
          />
        ) : null}
        {visibleMetrics.includes("generationSpeed") ? (
          <UsageMetric
            icon={<TrendingUp aria-hidden />}
            label={t("usage.generationSpeed")}
            value={formatTokenSpeed(generationSpeed, locale, speedUnit)}
            title={t("usage.generationSpeedHint")}
          />
        ) : null}
        {visibleMetrics.includes("e2eSpeed") ? (
          <UsageMetric
            icon={<Gauge aria-hidden />}
            label={t("usage.summaryMetrics.e2eSpeed")}
            value={formatTokenSpeed(e2eSpeed, locale, speedUnit)}
          />
        ) : null}
      </div>
    </section>
  );
}
