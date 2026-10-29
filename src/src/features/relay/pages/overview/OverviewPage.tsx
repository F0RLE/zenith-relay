import { startTransition, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Activity, ArrowRight, CircleAlert, Play, Square, Users } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { LaunchApplication } from "../../api/types";
import { Button, PageHeader } from "../../components/Ui";
import { ApplicationPickerDialog } from "../../components/ApplicationPickerDialog";
import { useRelayState } from "../../state/RelayStateProvider";
import { useSavedChoice } from "../../state/usePendingFlag";
import { useRelayUsageContext } from "../../state/relayStateContext";
import { readRelayPreference, removeRelayPreference, writeRelayPreference } from "../../state/relayPreferences";
import { emptyUsageTotals, formatCompactNumber } from "../../usageTotals";
import { getCachedOverviewAnalytics, isOverviewAnalyticsFresh, loadOverviewAnalytics } from "./overviewAnalyticsCache";
import { analyticsFromPage, chartWindows, DAY_MS, HOUR_MS, localSamples, type Analytics, type AnalyticsScope, type Range } from "./overviewAnalyticsModel";
import AnalyticsPanel from "./OverviewAnalytics";
import WeeklyCapacityChart from "./WeeklyCapacityChart";

export function OverviewPage() {
  const { t, i18n } = useTranslation();
  const { mode, runtime, runtimeReady, perform, busy } = useRelayState();
  const { usageRevision } = useRelayUsageContext();
  const [applicationDialog, setApplicationDialog] = useState(false);
  const [range, setRange] = useState<Range>("today");
  const [analyticsScopeSelection, setAnalyticsScopeSelection] = useState<AnalyticsScope>(() => {
    const stored = readRelayPreference("relay.overviewAnalyticsScope", "");
    return stored?.startsWith("source:") || stored?.startsWith("account:") ? stored as AnalyticsScope : "";
  });
  const [analyticsLoading, setAnalyticsLoading] = useState(false);
  const [analyticsError, setAnalyticsError] = useState(false);
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const now = new Date();
  const windowRevision = range === "today"
    ? Math.floor(now.getTime() / HOUR_MS)
    : `${now.getFullYear()}-${now.getMonth()}-${now.getDate()}`;
  const windows = useMemo(() => chartWindows(range, locale), [range, locale, windowRevision]);
  const usageAvailable = mode === "local";
  const windowStartMs = windows[0]?.startMs ?? 0;
  const windowEndMs = windows[windows.length - 1]?.endMs ?? windowStartMs;
  const runtimeIdentity = mode;
  const usageScope = JSON.stringify([runtimeIdentity, range, analyticsScopeSelection, windowStartMs, windowEndMs]);
  const [overviewData, setOverviewData] = useState<{ scope: string; data: Analytics | null }>(() => ({ scope: usageScope, data: getCachedOverviewAnalytics(usageScope) }));
  const analyticsScope = useRef<string | null>(null);
  const appliedUsageRevision = useRef(usageRevision);
  const analyticsScopeQuery = analyticsScopeSelection
    ? analyticsScopeSelection.slice(analyticsScopeSelection.indexOf(":") + 1)
    : undefined;
  const analyticsScopeOptions = useMemo(() => [
    { value: "", label: t("overview.scopeAll") },
    ...(runtime?.sources ?? []).map((apiSource) => ({ value: `source:${apiSource.id}`, label: `${t("overview.scopeApi")} · ${apiSource.name}` })),
    ...(runtime?.accounts ?? []).map((account) => ({ value: `account:${account.id}`, label: `${t("overview.scopeAccount")} · ${account.label}` })),
  ], [runtime?.accounts, runtime?.sources, t]);
  const analytics = overviewData.scope === usageScope ? overviewData.data : getCachedOverviewAnalytics(usageScope);
  const gatewayRunning = useSavedChoice(Boolean(runtime?.gateway.running));
  const running = gatewayRunning.value;
  const setAnalyticsScope = useCallback((scopeValue: string) => {
    const selectedScope = scopeValue as AnalyticsScope;
    setAnalyticsScopeSelection(selectedScope);
    writeRelayPreference("relay.overviewAnalyticsScope", selectedScope);
  }, []);
  const launchApplication = (application: LaunchApplication) => perform(`${application}-launch`, () => relayCommands.launchApplication(application), "feedback.launched", {
    skipRefresh: true,
  });

  useEffect(() => {
    if (!runtimeReady) return;
    if (analyticsScopeSelection && !analyticsScopeOptions.some((option) => option.value === analyticsScopeSelection)) {
      setAnalyticsScopeSelection("");
      removeRelayPreference("relay.overviewAnalyticsScope");
    }
  }, [analyticsScopeOptions, analyticsScopeSelection, runtimeReady]);

  useEffect(() => {
    const controller = new AbortController();
    const { signal } = controller;
    const scopeChanged = analyticsScope.current !== usageScope;
    analyticsScope.current = usageScope;
    if (scopeChanged) {
      // Re-entry can reuse the last complete snapshot immediately. The same
      // scope is refreshed below, so the cached value is only a stale view.
      const cached = getCachedOverviewAnalytics(usageScope);
      startTransition(() => setOverviewData({ scope: usageScope, data: cached }));
    }
    setAnalyticsError(false);
    const stopLoading = () => {
      setAnalyticsLoading(false);
    };
    if (!runtimeReady) {
      stopLoading();
      return () => controller.abort();
    }
    if (!usageAvailable) {
      startTransition(() => setOverviewData({ scope: usageScope, data: null }));
      stopLoading();
      return () => controller.abort();
    }
    const usageChanged = appliedUsageRevision.current !== usageRevision;
    appliedUsageRevision.current = usageRevision;
    if (!usageChanged && isOverviewAnalyticsFresh(usageScope)) {
      stopLoading();
      return () => controller.abort();
    }
    setAnalyticsLoading(true);
    const usageQuery = {
      page: 1,
      pageSize: 1,
      range: "custom" as const,
      fromMs: windowStartMs,
      toMs: windowEndMs,
      bucketMs: range === "today" ? HOUR_MS : DAY_MS,
      includeEvents: false,
      includeModels: false,
      includePoolMembers: false,
      ...(analyticsScopeQuery ? { sourceOrAccountQuery: analyticsScopeQuery } : {}),
    };
    const load = () => relayCommands.localUsagePage(usageQuery).then((page) => analyticsFromPage(page.totals, page.buckets, localSamples(page.events), windows));
    loadOverviewAnalytics(usageScope, load, usageChanged, signal)
      .then((analyticsResult) => {
        if (!analyticsResult) {
          if (!signal.aborted) setAnalyticsError(true);
          return;
        }
        if (signal.aborted) return;
        startTransition(() => setOverviewData({ scope: usageScope, data: analyticsResult }));
      })
      .catch(() => !signal.aborted && setAnalyticsError(true))
      .finally(() => {
        if (!signal.aborted) setAnalyticsLoading(false);
      });
    return () => controller.abort();
  }, [mode, range, runtimeReady, usageRevision, usageAvailable, usageScope, analyticsScopeQuery, windowEndMs, windowStartMs, windows]);

  const totals = analytics?.totals ?? emptyUsageTotals();
  const requests = totals.requests;
  const models = runtime?.gateway.visibleModelIds.length ?? 0;
  const healthy = [...(runtime?.sources ?? []), ...(runtime?.accounts ?? []).filter((account) => !account.routeInactive)].filter((poolMember) => poolMember.enabled).length;
  const errors = Math.max(0, totals.requests - totals.successfulRequests);

  const primary = <>
    <Button
      variant={running ? "secondary" : "primary"}
      busy={busy === "gateway"}
      icon={running ? <Square aria-hidden /> : <Play aria-hidden />}
      onClick={() => {
        const shouldRunGateway = !running;
        void perform("gateway", async () => {
          if (shouldRunGateway) await relayCommands.startGateway();
          else await relayCommands.stopGateway();
          gatewayRunning.confirm(shouldRunGateway);
        }, shouldRunGateway ? "feedback.started" : "feedback.stopped", { backgroundRefresh: true });
      }}
    >
      {running ? t("gateway.stop") : t("gateway.start")}
    </Button>
    <Button
      variant={running ? "primary" : "secondary"}
      busy={busy === "chatgpt-launch" || busy === "opencode-launch" || busy === "claude-desktop-launch"}
      icon={<Play aria-hidden />}
      onClick={() => setApplicationDialog(true)}
    >
      {t("overview.launchApplication")}
    </Button>
  </>;

  return <section className="relay-page"><PageHeader title={t("nav.overview")} actions={runtimeReady ? primary : undefined} />
      <div className="metric-band overview-metrics">
        <div><Activity aria-hidden /><span>{t("overview.requestsToday")}</span><strong>{analytics ? formatCompactNumber(requests, locale) : "—"}</strong></div>
        <div><Users aria-hidden /><span>{t("overview.healthy")}</span><strong>{runtime ? healthy : "—"}</strong></div>
        <div><ArrowRight aria-hidden /><span>{t("overview.models")}</span><strong>{runtime ? models || "-" : "—"}</strong></div>
        <div><CircleAlert aria-hidden /><span>{t("overview.errors")}</span><strong>{analytics ? formatCompactNumber(errors, locale) : "—"}</strong></div>
      </div>
      <AnalyticsPanel
        range={range}
        setRange={setRange}
        windows={windows}
        analytics={analytics}
        loading={analyticsLoading || (!analytics && !analyticsError)}
        error={analyticsError}
        scope={analyticsScopeSelection}
        setScope={setAnalyticsScope}
        scopeOptions={analyticsScopeOptions}
        capacity={mode === "local" && runtimeReady && !analyticsScopeSelection.startsWith("source:")
          ? <WeeklyCapacityChart windows={windows} revision={runtime?.weeklyCapacityRevision ?? 0} {...(analyticsScopeQuery ? { accountId: analyticsScopeQuery } : {})} /> : null}
      />
    {applicationDialog ? (
      <ApplicationPickerDialog
        title={t("overview.applicationPickerTitle")}
        onClose={() => setApplicationDialog(false)}
        onChatGPT={() => void launchApplication("chatgpt")}
        onOpenCode={() => void launchApplication("opencode")}
        onClaudeDesktop={() => void launchApplication("claude-desktop")}
      />
    ) : null}
  </section>;
}
