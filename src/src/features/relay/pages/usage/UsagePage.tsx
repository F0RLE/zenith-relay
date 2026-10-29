import "../../../../styles/pages/usage.css";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { CalendarDays, Download, SlidersHorizontal, Trash2, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { UsageQuery, UsageTotals } from "../../api/types";
import { ActionMenu, ActionMenuItem, Button, IconButton, OptionMenu, PageHeader, Tabs, useConfirm } from "../../components/Ui";
import { orderModelIdsBySnapshot } from "../../modelGroups";
import { useRelayState } from "../../state/RelayStateProvider";
import { useRelayUsageContext } from "../../state/relayStateContext";
import { formatTokenSpeed, observedTokensPerSecond } from "../../usageSpeed";
import { AggregateView } from "./AggregateView";
import { ErrorsView } from "./ErrorsView";
import { RequestDetailsHost, type RequestDetailsHandle } from "./RequestDetailsHost";
import { RequestFilters } from "./RequestFilters";
import { RequestsView } from "./RequestsView";
import { requestStatusKind } from "./usageReportFormat";
import { CompactNumber } from "./usageReportParts";
import { UsageMetric } from "./UsageMetric";
import { UsagePagination } from "./UsagePagination";
import { AccountUsageSummary } from "./AccountUsageSummary";
import { physicalRequestRows, totalsFromRows, usageRowsFromLocal, type UsageRow } from "./usageData";
import { formatUsageApiEquivalent } from "./usageFormatting";
import { formatFullNumber } from "../../usageTotals";
import { clearOverviewAnalytics } from "../overview/overviewAnalyticsCache";
import { UsageWorkloadMenu, type UsageWorkloadSelection } from "../../components/UsageWorkloadMenu";
import { useLazyUsageFilterOptions } from "./useLazyUsageFilterOptions";

type View = "requests" | "models" | "connections" | "errors";
type Range = "all" | "daily" | "weekly" | "monthly";
const USAGE_SUMMARY_METRICS = ["requests", "success", "tokens", "equivalent", "generationSpeed", "e2eSpeed"] as const;
type UsageSummaryMetric = typeof USAGE_SUMMARY_METRICS[number];
const REQUEST_SUMMARY_METRICS: UsageSummaryMetric[] = [...USAGE_SUMMARY_METRICS];
const AGGREGATE_SUMMARY_METRICS: UsageSummaryMetric[] = ["requests", "tokens", "equivalent"];
const REQUEST_SEARCH_DEBOUNCE_MS = 250;

export function UsagePage() {
  const { t, i18n } = useTranslation();
  const { mode, runtime, busy, perform, accountDisplayName } = useRelayState();
  const { usageRevision, localUsagePage, localUsageQueryKey, getCachedLocalUsage, loadLocalUsage, invalidateLocalUsage } = useRelayUsageContext();
  const confirm = useConfirm();
  const [view, setView] = useState<View>("requests");
  const [status, setStatus] = useState("all");
  const [range, setRange] = useState<Range>("all");
  const [workload, setWorkload] = useState<UsageWorkloadSelection>("");
  const [modelQuery, setModelQuery] = useState("");
  const [connectionQuery, setConnectionQuery] = useState("");
  const [wireApi, setWireApi] = useState("");
  const [transport, setTransport] = useState("");
  const [errorQuery, setErrorQuery] = useState("");
  const [requestQuery, setRequestQuery] = useState("");
  const [committedRequestQuery, setCommittedRequestQuery] = useState("");
  const [showMoreFilters, setShowMoreFilters] = useState(false);
  const [page, setPage] = useState(1);
  const [usageLoading, setUsageLoading] = useState(false);
  const [usageError, setUsageError] = useState(false);
  const [retryRevision, setRetryRevision] = useState(0);
  const detailsRef = useRef<RequestDetailsHandle>(null);
  const selectRequest = useCallback((row: UsageRow) => detailsRef.current?.open(row), []);
  const appliedRevision = useRef({ usage: usageRevision, retry: retryRevision });
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const usageSupported = mode === "local";
  const runtimeReady = runtime !== null;
  const requestFiltersActive = view === "requests";
  const requestSearchPending = requestFiltersActive && requestQuery.trim() !== committedRequestQuery;
  useEffect(() => {
    if (!requestFiltersActive || !requestSearchPending) return;
    const query = requestQuery.trim();
    const commit = () => {
      setPage(1);
      detailsRef.current?.close();
      setCommittedRequestQuery(query);
    };
    if (!query) {
      commit();
      return;
    }
    const timer = window.setTimeout(commit, REQUEST_SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [requestFiltersActive, requestQuery, requestSearchPending]);
  const usageQuery = useMemo<UsageQuery>(() => {
    const model = modelQuery.trim();
    const connection = connectionQuery.trim();
    const error = requestFiltersActive ? errorQuery.trim() : "";
    const requestId = requestFiltersActive ? committedRequestQuery : "";
    const selectedWireApi: NonNullable<UsageQuery["wireApi"]> | undefined = requestFiltersActive && wireApi
      ? wireApi as NonNullable<UsageQuery["wireApi"]>
      : undefined;
    const selectedTransport: NonNullable<UsageQuery["transport"]> | undefined = requestFiltersActive && transport
      ? transport as NonNullable<UsageQuery["transport"]>
      : undefined;
    const success = view === "errors" ? false : status !== "all" ? status === "success" : undefined;
    return {
      page,
      pageSize: 50,
      ...(range !== "all" ? { range } : {}),
      ...(workload ? { workload } : {}),
      ...(model ? { modelQuery: model } : {}),
      ...(connection ? { sourceOrAccountQuery: connection } : {}),
      ...(selectedWireApi !== undefined ? { wireApi: selectedWireApi } : {}),
      ...(selectedTransport !== undefined ? { transport: selectedTransport } : {}),
      ...(success !== undefined ? { success } : {}),
      ...(error ? { errorCategory: error } : {}),
      ...(requestId ? { requestIdQuery: requestId } : {}),
      // Totals accompany the selected view; detailed events still load only
      // for its current page, never for neighboring pages or aggregate tabs.
      includeEvents: view === "requests" || view === "errors",
      includeModels: view === "models",
      includePoolMembers: view === "connections",
      includeReport: true,
      includeFilterOptions: false,
    };
  }, [page, range, workload, modelQuery, connectionQuery, wireApi, transport, status, errorQuery, committedRequestQuery, view, requestFiltersActive]);
  const usageQueryKey = JSON.stringify(usageQuery);
  const usagePage = mode === "local"
    ? (localUsageQueryKey === usageQueryKey ? localUsagePage : getCachedLocalUsage(usageQuery))
    : null;
  const displayedUsagePage = useRef(usagePage);
  displayedUsagePage.current = usagePage;
  const filterChoices = useLazyUsageFilterOptions(usageQuery, usageRevision, usagePage?.filterOptions, () => setUsageError(true));
  const reportComplete = usagePage !== null && usagePage.hasMore === undefined;

  useEffect(() => {
    if (!runtimeReady || !usageSupported) {
      setUsageLoading(false);
      return;
    }
    if (requestSearchPending) return;
    const controller = new AbortController();
    const usageChanged = appliedRevision.current.usage !== usageRevision;
    const retryRequested = appliedRevision.current.retry !== retryRevision;
    appliedRevision.current = { usage: usageRevision, retry: retryRevision };
    // Live history updates replace the report in place. Keep the spinner and
    // navigation lock for explicit loads, not every completed request.
    setUsageLoading(!usageChanged || displayedUsagePage.current === null);
    setUsageError(false);
    void loadLocalUsage(usageQuery, {
      force: usageChanged || retryRequested, revision: usageRevision, signal: controller.signal,
    })
      .catch(() => !controller.signal.aborted && setUsageError(true))
      .finally(() => !controller.signal.aborted && setUsageLoading(false));
    return () => controller.abort();
  }, [mode, runtimeReady, usageRevision, retryRevision, usageSupported, usageQuery, usageQueryKey, loadLocalUsage, requestSearchPending]);
  useEffect(() => {
    setPage(1);
    detailsRef.current?.close();
  }, [mode]);

  const accountLabels = useMemo(() => new Map(runtime?.accounts.map((account) => [account.id, account.label]) ?? []), [runtime?.accounts]);
  const sourceLabels = useMemo(() => new Map(runtime?.sources.map((source) => [source.id, source.name]) ?? []), [runtime?.sources]);
  const rows = useMemo<UsageRow[]>(() => {
    const labels = {
      backgroundConnection: t("codex.backgroundConnection"),
      removedAccount: t("usage.removedAccount"),
      unknownConnection: t("common.unknown"),
    };
    return physicalRequestRows(usageRowsFromLocal(usagePage?.events ?? [], {
      ...labels,
      accountLabels,
      sourceLabels,
    }));
  }, [usagePage?.events, accountLabels, sourceLabels, t]);
  // Keep the last complete report during refresh; display empty results only
  // after a successful read.
  const awaitingReport = usageSupported && !usagePage && !usageError;
  const reportBusy = usageLoading || requestSearchPending || awaitingReport;
  const totals = usagePage?.totals ?? totalsFromRows(rows);
  const averageGenerationSpeed = observedTokensPerSecond(totals.generationOutputTokens, totals.generationMs);
  const averageE2eSpeed = observedTokensPerSecond(totals.speedOutputTokens, totals.speedDurationMs);
  const successRate = totals.requests ? Math.round(totals.successfulRequests / totals.requests * 100) : null;
  const timeFormatter = useMemo(() => new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "medium" }), [locale]);
  const formatTime = useCallback((timestamp: string) => timeFormatter.format(new Date(timestamp)), [timeFormatter]);
  const resetPage = (work: () => void) => { work(); setPage(1); detailsRef.current?.close(); };
  const changePage = (nextPage: number) => { setPage(nextPage); detailsRef.current?.close(); };
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
  }))), "feedback.exported", { skipRefresh: true });
  const clearLogs = async () => {
    if (!await confirm(t("usage.clearConfirm"), { danger: true })) return;
    setPage(1);
    await perform("usage-clear", async () => {
      await relayCommands.clearLocalUsage();
      clearOverviewAnalytics();
      await invalidateLocalUsage();
      setRetryRevision((revision) => revision + 1);
    }, "feedback.cleared", { backgroundRefresh: true });
  };
  const canClear = mode === "local";
  const modelGroups = usagePage?.models;
  const poolMemberGroups = useMemo(() => usagePage?.poolMembers?.map((group) => ({
    ...group,
    label: accountLabels.get(group.key) ?? sourceLabels.get(group.key) ?? group.label ?? t("common.unknown"),
  })), [accountDisplayName, accountLabels, mode, sourceLabels, t, usagePage?.poolMembers]);
  const modelOptionIds = useMemo(() => orderModelIdsBySnapshot(
    [
      ...(filterChoices.options?.models ?? runtime?.gateway.models?.map((model) => model.id) ?? []),
      ...(modelQuery ? [modelQuery] : []),
    ],
    runtime?.gateway.models ?? [],
    runtime?.gateway.modelCatalog,
  ), [modelQuery, runtime?.gateway.models, runtime?.gateway.modelCatalog, filterChoices.options?.models]);
  const modelOptions = useMemo(() => [{ value: "", label: t("usage.anyModel") }, ...modelOptionIds.map((modelId) => ({ value: modelId, label: modelId }))], [modelOptionIds, t]);
  const poolMemberOptions = useMemo(() => [{ value: "", label: t("usage.anyPoolMember") },
    ...Array.from(new Set([
      ...(filterChoices.options?.poolMembers ?? [...accountLabels.keys(), ...sourceLabels.keys()]),
      ...(connectionQuery ? [connectionQuery] : []),
    ])).map((key) => ({ value: key, label: accountLabels.get(key) ?? sourceLabels.get(key) ?? key }))
      .sort((left, right) => left.label.localeCompare(right.label, i18n.language)),
  ], [accountLabels, connectionQuery, i18n.language, sourceLabels, t, filterChoices.options?.poolMembers]);
  const selectedAccount = runtime?.accounts.find((account) => account.id === connectionQuery);
  const errorRows = useMemo(() => rows.filter((usageRow) => requestStatusKind(usageRow) === "error"), [rows]);
  const clearFilters = () => {
    setRange("all"); setWorkload(""); setStatus("all"); setModelQuery(""); setConnectionQuery("");
    setWireApi(""); setTransport(""); setErrorQuery(""); setRequestQuery("");
    setCommittedRequestQuery("");
    setPage(1); detailsRef.current?.close();
  };
  const visibleSummaryMetrics = view === "requests" ? REQUEST_SUMMARY_METRICS : AGGREGATE_SUMMARY_METRICS;
  const showStatusFilter = view !== "errors";
  const showModelFilter = view !== "models";
  const showPoolMemberFilter = view !== "connections";
  const additionalFilterCount = [wireApi, transport, errorQuery, requestQuery.trim()].filter(Boolean).length;
  const scopeMenuProps = { showSelectionIndicator: false };
  const scopeFilterCount = 2 + Number(showStatusFilter) + Number(showModelFilter) + Number(showPoolMemberFilter);
  const hasFilters = range !== "all"
    || Boolean(workload)
    || (showStatusFilter && status !== "all")
    || (showModelFilter && Boolean(modelQuery))
    || (showPoolMemberFilter && Boolean(connectionQuery))
    || (requestFiltersActive && Boolean(wireApi || transport || errorQuery || requestQuery));
  const changeView = (nextView: View) => {
    setView(nextView);
    setPage(1);
    detailsRef.current?.close();
    setShowMoreFilters(false);
    if (nextView === "errors") setStatus("all");
    if (nextView === "models") setModelQuery("");
    if (nextView === "connections") setConnectionQuery("");
  };
  const loadFilterChoicesFor = (target: EventTarget) => {
    if (target instanceof Element && target.closest(".usage-model-menu, .usage-pool-member-menu, .usage-filter-panel .filter-option-menu")) filterChoices.load();
  };

  return <section className="relay-page relay-workspace-page usage-page" aria-busy={reportBusy}
    onClickCapture={(event) => loadFilterChoicesFor(event.target)}
    onKeyDownCapture={(event) => {
      if (["ArrowDown", "ArrowUp", "Enter", " "].includes(event.key)) loadFilterChoicesFor(event.target);
    }}>
    <PageHeader
      title={t("nav.usage")}
      navigation={<Tabs
        value={view}
        onChange={(nextView) => changeView(nextView as View)}
        label={t("usage.views")}
        items={[
          { id: "requests", label: t("usage.sends") },
          { id: "models", label: t("common.models") },
          { id: "connections", label: t("usage.poolMembers") },
          { id: "errors", label: t("overview.errors") },
        ]}
      />}
      actions={<>
        <ActionMenu className="usage-overflow">
          <ActionMenuItem icon={<Download aria-hidden />} disabled={!usagePage || reportBusy || busy === "usage-export"} onClick={exportRows}>{t("common.export")}</ActionMenuItem>
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
            { value: "all", label: t("usage.allTime") },
          ]}
        />
        <UsageWorkloadMenu value={workload} onChange={(next) => resetPage(() => setWorkload(next))} />
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
              onClick={() => { if (!showMoreFilters) filterChoices.load(); setShowMoreFilters((isVisible) => !isVisible); }}
            />
            {additionalFilterCount ? <small>{additionalFilterCount}</small> : null}
          </span> : null}
        </div> : null}
        {hasFilters ? <IconButton className="usage-clear-filters" label={t("usage.clearFilters")} icon={<X aria-hidden />} onClick={clearFilters} /> : null}
      </div>
    </div>
    {requestFiltersActive && showMoreFilters ? <RequestFilters
      filterOptions={filterChoices.options}
      wireApi={wireApi}
      onWireApiChange={(value) => resetPage(() => setWireApi(value))}
      transport={transport}
      onTransportChange={(value) => resetPage(() => setTransport(value))}
      errorQuery={errorQuery}
      onErrorChange={(value) => resetPage(() => setErrorQuery(value))}
      requestQuery={requestQuery}
      onRequestChange={setRequestQuery}
    /> : null}
    {usagePage ? <>
    {reportComplete && visibleSummaryMetrics.length ? (
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
    {reportComplete && selectedAccount && view !== "errors" ? <AccountUsageSummary account={selectedAccount} totals={totals} /> : null}
    {view === "requests" ? <RequestsView
      rows={rows}
      formatTime={formatTime}
      onSelect={selectRequest}
    /> : null}
    {view === "models" ? <AggregateView rows={rows} {...(modelGroups ? { groups: modelGroups } : {})} field="model" empty={t("usage.empty")} /> : null}
    {view === "connections" ? <AggregateView rows={rows} {...(poolMemberGroups ? { groups: poolMemberGroups } : {})} field="connection" empty={t("usage.empty")} /> : null}
    {view === "errors" ? <ErrorsView rows={errorRows} formatTime={formatTime} onSelect={selectRequest} /> : null}
    </> : null}
    {usageError ? <div role="alert" className="form-note error-text">
      {t("usage.loadFailed")}
      <Button variant="secondary" disabled={reportBusy} onClick={() => setRetryRevision((revision) => revision + 1)}>{t("common.retry")}</Button>
    </div> : null}
    {(view === "requests" || view === "errors") && usagePage && usagePage.page === page && (page > 1 || usagePage.hasMore || usagePage.totalPages > 1) ? (
      <UsagePagination
        key={JSON.stringify([mode, view, status, range, modelQuery, connectionQuery, wireApi, transport, errorQuery, committedRequestQuery])}
        page={page}
        totalPages={usagePage.totalPages}
        hasMore={usagePage.hasMore}
        loading={usageLoading || requestSearchPending}
        onPageChange={changePage}
      />
    ) : null}
    <RequestDetailsHost key={mode} ref={detailsRef} rows={rows} local={mode === "local"} />
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
        {visibleMetrics.includes("requests") ? <UsageMetric label={t("usage.clientRequests")} value={<CompactNumber value={totals.requests} locale={language} />} /> : null}
        {visibleMetrics.includes("success") ? (
          <UsageMetric
            className="usage-success-metric"
            label={t("common.success")}
            value={successRate == null ? "-" : `${successRate}%`}
            title={`${formatFullNumber(totals.successfulRequests, language)} / ${formatFullNumber(totals.requests, language)}`}
          />
        ) : null}
        {visibleMetrics.includes("tokens") ? (
          <UsageMetric
            className="usage-token-metric"
            label={t("usage.totalTokens")}
            value={<CompactNumber value={totals.totalTokens} locale={language} />}
            title={t("usage.tokenCompositionHint")}
          />
        ) : null}
        {visibleMetrics.includes("equivalent") ? (
          <UsageMetric
            label={t("usage.apiEquivalent")}
            value={formatUsageApiEquivalent(totals.apiEquivalent, language, totals.apiEquivalent.microUsd >= 1_000_000 ? 2 : 4)}
            title={[
              formatUsageApiEquivalent(totals.apiEquivalent, language),
              totals.apiEquivalent.unpricedTokens > 0 ? t("usage.apiEquivalentCoverage", {
                priced: formatFullNumber(totals.apiEquivalent.pricedTokens, language),
                unpriced: formatFullNumber(totals.apiEquivalent.unpricedTokens, language),
              }) : null,
              t("usage.apiEquivalentHint", { count: formatFullNumber(totals.apiEquivalent.unpricedTokens, language) }),
            ].filter(Boolean).join(" · ")}
          />
        ) : null}
      </div>
      <div className="usage-overview-details">
        {visibleMetrics.includes("tokens") ? <dl className="usage-token-totals">
          <div><dt>{t("usage.inputTokens")}</dt><dd><CompactNumber value={totals.inputTokens} locale={language} /></dd></div>
          <div><dt>{t("usage.outputTokens")}</dt><dd><CompactNumber value={totals.outputTokens} locale={language} /></dd></div>
          <div><dt>{t("usage.cachedShort")}</dt><dd>{totals.cachedInputSamples ? <CompactNumber value={totals.cachedInputTokens} locale={language} /> : "—"}</dd></div>
          {totals.cacheWriteInputSamples ? <div><dt>{t("usage.cacheWriteShort")}</dt><dd><CompactNumber value={totals.cacheWriteInputTokens ?? 0} locale={language} /></dd></div> : null}
        </dl> : null}
        {visibleMetrics.includes("generationSpeed") || visibleMetrics.includes("e2eSpeed") ? <div className="usage-secondary-metrics">
        {visibleMetrics.includes("generationSpeed") ? (
          <UsageMetric
            label={t("usage.generationSpeed")}
            value={formatTokenSpeed(generationSpeed, locale, speedUnit)}
            title={t("usage.generationSpeedHint")}
          />
        ) : null}
        {visibleMetrics.includes("e2eSpeed") ? (
          <UsageMetric
            label={t("usage.summaryMetrics.e2eSpeed")}
            value={formatTokenSpeed(e2eSpeed, locale, speedUnit)}
          />
        ) : null}
        </div> : null}
      </div>
    </section>
  );
}
