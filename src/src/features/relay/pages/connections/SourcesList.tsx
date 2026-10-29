import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowDown, ArrowUp, CircleAlert, CircleCheck, CirclePause, Cloud, ListMinus, ListPlus, Pencil, Play, Power, RefreshCw, Trash2, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { CandidateRuntimeSnapshot, SourceSummary } from "../../api/types";
import { ApplicationPickerDialog } from "../../components/ApplicationPickerDialog";
import { SourceStatsPanel } from "../../components/SourceStatsPanel";
import { formatDetailedRemainingTime } from "../../quotaFormatting";
import { sourceSupportsNativeResponses, sourceSupportsNativeProtocol } from "../../sourceProtocolBindings";
import { ActionMenu, ActionMenuItem, EmptyState, ErrorDetailsDialog, IconButton, OptionMenu, StatusIcon, useConfirm } from "../../components/Ui";
import type { FeedbackError } from "../../state/feedback";
import { useRelayState } from "../../state/RelayStateProvider";
import { setVisibleIdsSelected, toggleSelectedId } from "../../state/selectionModel";
import { NoResults } from "./connectionHelpers";
import { routingOrderPositions, upcomingModelRetries } from "../../routingOrder";
import { createRuntimeCandidateLookup } from "../../runtimeCandidateIndex";
import { updatePoolMembership } from "../../poolMembership";
import { useRelativeTimeClock } from "../../hooks/useRelativeTimeClock";
import type { SourceStatsState } from "../../sourceStatsModel";
import { AccountCardLayout } from "./AccountCard";
import { filterAndSortSources, sourceStatus, type SourceSortKey, type SourceSortDirection } from "./sourceListModel";
import { sourceErrorDetails } from "./sourceError";

const EMPTY_SOURCES: SourceSummary[] = [];
const EMPTY_RUNTIME_ORDER: CandidateRuntimeSnapshot[] = [];

export function SourcesList({ query, onQuery, sourceStats, refreshReport, onEdit, onRefresh, onRefreshSelected, onRefreshAll }: {
  query: string;
  onQuery: (query: string) => void;
  sourceStats: Record<string, SourceStatsState>;
  refreshReport: string | null;
  onEdit: (source: SourceSummary) => void;
  onRefresh: (sourceId: string) => void;
  onRefreshSelected: (sourceIds: readonly string[]) => void;
  onRefreshAll: () => void;
}) {
  const { t } = useTranslation();
  const { mode, runtime, perform, refresh, activateCodexProfile, busy } = useRelayState();
  const confirm = useConfirm();
  const [sort, setSort] = useState<{ key: SourceSortKey; direction: SourceSortDirection }>({ key: "runtime", direction: "asc" });
  const [pendingPool, setPendingPool] = useState<Record<string, boolean>>({});
  const [pendingEnabled, setPendingEnabled] = useState<Record<string, boolean>>({});
  const [launchSourceId, setLaunchSourceId] = useState<string | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [errorDetails, setErrorDetails] = useState<FeedbackError | null>(null);
  const selectAllRef = useRef<HTMLInputElement>(null);
  const sourcesSnapshot = runtime?.sources ?? EMPTY_SOURCES;
  useEffect(() => {
    setSelected((previous) => previous.filter((id) => sourcesSnapshot.some((source) => source.id === id)));
  }, [sourcesSnapshot]);
  useEffect(() => {
    setSelected([]);
    setErrorDetails(null);
    setLaunchSourceId(null);
    setPendingPool({});
    setPendingEnabled({});
  }, [mode]);
  const membershipSignature = sourcesSnapshot.map((source) => `${source.id}:${source.inPool}:${source.enabled}`).join("|");
  useEffect(() => {
    const savedPool = new Map(sourcesSnapshot.map((source) => [source.id, source.inPool]));
    const savedEnabled = new Map(sourcesSnapshot.map((source) => [source.id, source.enabled]));
    setPendingPool((previousPendingPool) => dropConfirmedFlags(previousPendingPool, savedPool));
    setPendingEnabled((previousPendingEnabled) => dropConfirmedFlags(previousPendingEnabled, savedEnabled));
  }, [membershipSignature]);
  const runtimeOrder = runtime?.gateway.routingOrder ?? EMPTY_RUNTIME_ORDER;
  const runtimeBySource = useMemo(() => {
    const runtimeFor = createRuntimeCandidateLookup(runtimeOrder);
    return new Map(sourcesSnapshot.map((source) => [source.id, runtimeFor(source.id, "api_source", "all", source.wireApi)]));
  }, [runtimeOrder, sourcesSnapshot]);
  const retryTimestamps = useMemo(() => runtimeOrder
    .flatMap((candidate) => candidate.kind === "api_source" ? [candidate.nextRetryAtMs] : []), [runtimeOrder]);
  const nowMs = useRelativeTimeClock(retryTimestamps);
  const runtimePosition = useMemo(() => routingOrderPositions(runtimeOrder), [runtimeOrder]);
  const sources = useMemo(
    () => filterAndSortSources(sourcesSnapshot, query, sort.key, sort.direction, runtimePosition),
    [query, runtimePosition, sort.direction, sort.key, sourcesSnapshot],
  );
  const selectedSources = sourcesSnapshot.filter((source) => selected.includes(source.id));
  const selectedVisibleCount = sources.filter((source) => selected.includes(source.id)).length;
  const allVisibleSelected = sources.length > 0 && selectedVisibleCount === sources.length;
  useEffect(() => {
    if (selectAllRef.current) selectAllRef.current.indeterminate = selectedVisibleCount > 0 && !allVisibleSelected;
  }, [selectedVisibleCount, allVisibleSelected]);
  if (!runtime?.sources.length) {
    return <EmptyState title={t("sources.emptyTitle")} description={t("sources.emptyDescription")} />;
  }
  const localSource = mode === "local";
  const inPoolCount = sourcesSnapshot.filter((source) => pendingPool[source.id] ?? source.inPool).length;
  const disabledCount = sourcesSnapshot.filter((source) => !(pendingEnabled[source.id] ?? source.enabled)).length;
  const errorCount = sourcesSnapshot.filter((source) => sourceStatus(
    source,
    (pendingPool[source.id] ?? source.inPool) ? runtimeBySource.get(source.id) : undefined,
    nowMs,
    pendingEnabled[source.id] ?? source.enabled,
  ).tone === "error").length;
  const launchSource = launchSourceId ? sourcesSnapshot.find((source) => source.id === launchSourceId) ?? null : null;
  const sortColumn = (key: SourceSortKey) => setSort((previousSort) =>
    previousSort.key === key
      ? { key, direction: previousSort.direction === "asc" ? "desc" : "asc" }
      : { key, direction: "asc" },
  );
  const rememberFlag = (
    setFlag: (update: (previousFlags: Record<string, boolean>) => Record<string, boolean>) => void,
    id: string,
    flagValue: boolean,
  ) => {
    setFlag((previousFlags) => ({ ...previousFlags, [id]: flagValue }));
  };
  const rollbackFlag = (
    setFlag: (update: (previousFlags: Record<string, boolean>) => Record<string, boolean>) => void,
    id: string,
    flagValue: boolean,
  ) => {
    setFlag((previousFlags) => {
      if (previousFlags[id] !== flagValue) return previousFlags;
      const remainingFlags = { ...previousFlags };
      delete remainingFlags[id];
      return remainingFlags;
    });
  };
  const updateParticipation = (source: SourceSummary, inPool: boolean) => {
    rememberFlag(setPendingPool, source.id, inPool);
    void perform(
      `source-pool-${source.id}`,
      () => updatePoolMembership(mode, { accountIds: [], sourceIds: [source.id], inPool }),
      "feedback.saved",
      { backgroundRefresh: true },
    ).then((ok) => {
      if (!ok) rollbackFlag(setPendingPool, source.id, inPool);
    });
  };
  const updateEnabled = (source: SourceSummary, enabled: boolean) => {
    rememberFlag(setPendingEnabled, source.id, enabled);
    void perform(
      `toggle-${source.id}`,
      () => relayCommands.setSourceEnabled(source.id, enabled),
      "feedback.saved",
      { backgroundRefresh: true },
    ).then((ok) => {
      if (!ok) rollbackFlag(setPendingEnabled, source.id, enabled);
    });
  };
  const updateSelectedParticipation = async (inPool: boolean) => {
    const sourceIds = selectedSources
      .filter((source) => (pendingPool[source.id] ?? source.inPool) !== inPool)
      .map((source) => source.id);
    if (!sourceIds.length) return;
    for (const id of sourceIds) rememberFlag(setPendingPool, id, inPool);
    const ok = await perform(
      "sources-pool-selected",
      () => updatePoolMembership(mode, { accountIds: [], sourceIds, inPool }),
      "feedback.saved",
      { backgroundRefresh: true },
    );
    if (!ok) for (const id of sourceIds) rollbackFlag(setPendingPool, id, inPool);
  };
  const updateSelectedEnabled = (enabled: boolean) => {
    const sourceIds = selectedSources
      .filter((source) => (pendingEnabled[source.id] ?? source.enabled) !== enabled)
      .map((source) => source.id);
    if (!sourceIds.length) return;
    void perform("sources-enabled-selected", async () => {
      try {
        for (const id of sourceIds) {
          await relayCommands.setSourceEnabled(id, enabled);
        }
      } catch (cause) {
        // Earlier commands may have succeeded; show the saved state before reporting the failure.
        await refresh().catch(() => undefined);
        throw cause;
      }
    }, "feedback.saved");
  };
  const deleteSelected = async () => {
    const sourceIds = selectedSources.map((source) => source.id);
    if (!sourceIds.length || !await confirm(t("sources.deleteSelectedConfirm", { count: sourceIds.length }), { danger: true })) return;
    await perform("sources-delete-selected", async () => {
      try {
        for (const id of sourceIds) {
          await relayCommands.deleteSource(id);
          setSelected((previous) => previous.filter((selectedId) => selectedId !== id));
        }
      } catch (cause) {
        await refresh().catch(() => undefined);
        throw cause;
      }
    }, "feedback.deleted");
  };
  return (
    <div className="sources-workspace">
      <div className="workspace-controls sources-controls">
        <div className="relay-status-summary" aria-label={t("sources.summaryLabel")}>
          <div><Cloud aria-hidden /><strong>{sourcesSnapshot.length}</strong><span>{t("accounts.summary.total")}</span></div>
          <div data-tone={inPoolCount ? "ready" : "muted"}><CircleCheck aria-hidden /><strong>{inPoolCount}</strong><span>{t("accounts.summary.inPool")}</span></div>
          <div data-tone={errorCount ? "error" : "muted"}><CircleAlert aria-hidden /><strong>{errorCount}</strong><span>{t("accounts.summary.errors")}</span></div>
          <div data-tone="muted"><CirclePause aria-hidden /><strong>{disabledCount}</strong><span>{t("accounts.summary.disabled")}</span></div>
        </div>
        <div className={`connection-command-bar source-command-bar${selectedSources.length ? " sources-selection-toolbar" : ""}`} data-selection={selectedSources.length > 0}>
          <div className="connection-command-context">
            <input
              ref={selectAllRef}
              type="checkbox"
              checked={allVisibleSelected}
              disabled={!sources.length || Boolean(busy)}
              aria-label={t("sources.selectVisible")}
              onChange={(event) => setSelected((selectedIds) => setVisibleIdsSelected(
                selectedIds, sources.map((source) => source.id), event.target.checked,
              ))}
            />
            {selectedSources.length ? <span>{t("sources.selectedCount", { count: selectedSources.length })}</span> : <label className="search-field">
              <span className="sr-only">{t("common.search")}</span>
              <input value={query} onChange={(event) => onQuery(event.target.value)} placeholder={t("common.search")} />
            </label>}
          </div>
          {!selectedSources.length ? <><div className="sources-sort-controls">
          <OptionMenu
            label={t("sources.sortLabel")}
            value={sort.key}
            onChange={(key) => setSort({ key: key as SourceSortKey, direction: "asc" })}
            options={[
              { value: "runtime", label: t("sources.sortDefault") },
              { value: "status", label: t("common.status") },
              { value: "name", label: t("common.name") },
              { value: "server", label: t("sources.host") },
              { value: "models", label: t("common.models") },
            ].map((option) => ({ ...option, shortLabel: `${t("sources.sortLabel")}: ${option.label}` }))}
          />
          {sort.key !== "runtime" ? (
            <IconButton
              label={t(sort.direction === "asc" ? "sources.sortDescending" : "sources.sortAscending", { column: t("sources.sortLabel") })}
              icon={sort.direction === "asc" ? <ArrowUp aria-hidden /> : <ArrowDown aria-hidden />}
              onClick={() => sortColumn(sort.key)}
            />
          ) : null}
          </div>
          <IconButton label={t("sources.refreshData")} icon={<RefreshCw aria-hidden />} busy={busy === "sources-refresh-all"} disabled={Boolean(busy)} onClick={onRefreshAll} />
          </> : <div className="sources-selection-actions">
            <IconButton label={t("sources.addToPoolAction")} icon={<ListPlus aria-hidden />} disabled={Boolean(busy) || selectedSources.every((source) => pendingPool[source.id] ?? source.inPool)} onClick={() => void updateSelectedParticipation(true)} />
            <IconButton label={t("sources.removeFromPoolAction")} icon={<ListMinus aria-hidden />} disabled={Boolean(busy) || selectedSources.every((source) => !(pendingPool[source.id] ?? source.inPool))} onClick={() => void updateSelectedParticipation(false)} />
            <ActionMenu label={t("common.actions")}>
              <ActionMenuItem icon={<RefreshCw aria-hidden />} disabled={Boolean(busy)} onClick={() => onRefreshSelected(selectedSources.map((source) => source.id))}>{t("sources.refreshData")}</ActionMenuItem>
              <ActionMenuItem icon={<Power aria-hidden />} disabled={Boolean(busy) || selectedSources.every((source) => pendingEnabled[source.id] ?? source.enabled)} onClick={() => updateSelectedEnabled(true)}>{t("common.enable")}</ActionMenuItem>
              <ActionMenuItem icon={<Power aria-hidden />} disabled={Boolean(busy) || selectedSources.every((source) => !(pendingEnabled[source.id] ?? source.enabled))} onClick={() => updateSelectedEnabled(false)}>{t("common.disable")}</ActionMenuItem>
              <ActionMenuItem danger icon={<Trash2 aria-hidden />} disabled={Boolean(busy)} onClick={() => void deleteSelected()}>{t("common.delete")}</ActionMenuItem>
            </ActionMenu>
          <IconButton label={t("accounts.clearSelection")} icon={<X aria-hidden />} disabled={Boolean(busy)} onClick={() => setSelected([])} />
          </div>}
        </div>
      </div>
        {refreshReport ? <div className="connection-toolbar-result" role="status">{refreshReport}</div> : null}
        {!sources.length ? <NoResults /> : null}
        <div className="account-list source-cards" role="list">{sources.map((source) => {
          const sourceState = sourceStats[source.id];
          const inPool = pendingPool[source.id] ?? source.inPool;
          const enabled = pendingEnabled[source.id] ?? source.enabled;
          const supportsNative = sourceSupportsNativeProtocol(source);
          const launchDisabled = !localSource || !supportsNative || !enabled || !source.secretAvailable || Boolean(busy);
          const launchTitle = !localSource
            ? t("sources.launchLocalOnly")
            : !supportsNative
              ? t("sources.launchNativeOnly")
              : !enabled || !source.secretAvailable
                ? t("sources.launchUnavailable")
                : t("sources.launch");
          const runtimeState = inPool
             ? runtimeBySource.get(source.id)
            : undefined;
          const status = sourceStatus(source, runtimeState, nowMs, enabled);
          const modelRetries = upcomingModelRetries(runtimeState, nowMs);
          const firstModelRetry = modelRetries[0];
          const runtimeHint = runtimeState?.halfOpen
            ? t("pool.recoveryProbe")
             : firstModelRetry
               ? t("pool.modelRetryAt", {
                 models: modelRetries.map((retry) => retry.model).join(", "),
                 time: formatDetailedRemainingTime(firstModelRetry.retryAtMs, nowMs, t),
               })
            : runtimeState?.nextRetryAtMs != null && runtimeState.nextRetryAtMs > nowMs
              ? t("pool.retryAt", { time: formatDetailedRemainingTime(runtimeState.nextRetryAtMs, nowMs, t) })
              : null;
          const failure = sourceErrorDetails(source.lastErrorCode, t);
          const statusLabel = t(`sources.status.${status.key}`);
          const indicatorLabel = [statusLabel, runtimeHint, failure?.summary].filter(Boolean).join(" · ");
          const indicatorTone = status.tone;
          return <AccountCardLayout
            key={source.id}
            className="source-card"
            data-source-id={source.id}
            data-enabled={enabled}
            data-in-pool={inPool}
            data-selected={selected.includes(source.id)}
            label={source.name}
            identityVisible
            selected={selected.includes(source.id)}
            selectionDisabled={Boolean(busy)}
            onToggleSelected={() => setSelected((selectedIds) => toggleSelectedId(selectedIds, source.id))}
            icon={failure
              ? <IconButton
                className="account-kind-icon account-status-button"
                data-status={indicatorTone}
                label={`${t("feedback.errorDetails")}: ${failure.summary}`}
                title={indicatorLabel}
                icon={<Cloud aria-hidden />}
                onClick={() => setErrorDetails(failure.error)}
              />
              : <StatusIcon className="account-kind-icon" status={indicatorTone} label={indicatorLabel}><Cloud aria-hidden /></StatusIcon>}
            details={<div className="source-card-host" data-relay-tooltip={source.baseUrl}><span>{source.baseUrl}</span></div>}
            menu={
              <ActionMenu className="account-row-menu">
                <ActionMenuItem
                  icon={<Power aria-hidden />}
                  disabled={Boolean(busy)}
                  onClick={() => void updateEnabled(source, !enabled)}
                >
                  {enabled ? t("common.disable") : t("common.enable")}
                </ActionMenuItem>
                <ActionMenuItem
                  danger
                  icon={<Trash2 aria-hidden />}
                  disabled={Boolean(busy)}
                  onClick={() => void confirm(t("sources.deleteConfirm"), { danger: true }).then((accepted) => accepted && perform(
                    `delete-${source.id}`,
                    () => relayCommands.deleteSource(source.id),
                    "feedback.deleted",
                    { backgroundRefresh: true },
                  ))}
                >
                  {t("common.delete")}
                </ActionMenuItem>
              </ActionMenu>
            }
            actions={<>
              <IconButton
                className={inPool ? "danger" : ""}
                label={t(inPool ? "sources.removeFromPoolAction" : "sources.addToPoolAction")}
                icon={inPool ? <ListMinus aria-hidden /> : <ListPlus aria-hidden />}
                disabled={Boolean(busy)}
                onClick={() => void updateParticipation(source, !inPool)}
              />
              <IconButton
                label={t("sources.refreshData")}
                icon={<RefreshCw aria-hidden />}
                busy={busy === `source-refresh-${source.id}`}
                disabled={Boolean(busy) || !source.secretAvailable}
                onClick={() => onRefresh(source.id)}
              />
              <IconButton
                label={t("common.edit")}
                icon={<Pencil aria-hidden />}
                disabled={Boolean(busy)}
                onClick={() => onEdit(source)}
              />
              <IconButton
                label={t("sources.launch")}
                title={launchTitle}
                icon={<Play aria-hidden />}
                disabled={launchDisabled}
                onClick={() => setLaunchSourceId(source.id)}
              />
            </>}
          >
            <SourceStatsPanel source={source} {...(sourceState ? { state: sourceState } : {})} />
          </AccountCardLayout>;
        })}</div>
      {errorDetails ? <ErrorDetailsDialog error={errorDetails} message={errorDetails.message} onClose={() => setErrorDetails(null)} /> : null}
      {launchSource ? <ApplicationPickerDialog
        title={t("sources.launchPickerTitle")}
        chatGPTDisabled={!sourceSupportsNativeResponses(launchSource)}
        onClose={() => setLaunchSourceId(null)}
        onChatGPT={() => {
          void activateCodexProfile(`launch-source-${launchSource.id}`, () => relayCommands.launchCodexSource(launchSource.id), true)
            .then((activated) => { if (activated) localStorage.setItem("relay.directSourceId", launchSource.id); });
        }}
        onOpenCode={() => {
          void perform(`launch-source-${launchSource.id}`, () => relayCommands.launchOpenCodeSource(launchSource.id), "feedback.launched", { backgroundRefresh: true })
            .then((launched) => { if (launched) localStorage.setItem("relay.directSourceId", launchSource.id); });
        }}
      /> : null}
    </div>
  );
}

function dropConfirmedFlags(pending: Record<string, boolean>, saved: ReadonlyMap<string, boolean>) {
  let changed = false;
  const remainingFlags = { ...pending };
  for (const [id, pendingValue] of Object.entries(pending)) {
    if (!saved.has(id) || saved.get(id) === pendingValue) {
      delete remainingFlags[id];
      changed = true;
    }
  }
  return changed ? remainingFlags : pending;
}
