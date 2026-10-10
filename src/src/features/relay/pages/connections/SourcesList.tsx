import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowDown, ArrowUp, CircleAlert, ListMinus, ListPlus, Loader2, Pencil, Play, Power, RefreshCw, Trash2, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { CandidateRuntimeSnapshot, SourceSummary } from "../../api/types";
import { operationalStatusTone, transientCandidateTone } from "../../accountStatus";
import { ApplicationPickerDialog } from "../../components/ApplicationPickerDialog";
import { formatDetailedRemainingTime } from "../../quotaFormatting";
import { sourceSupportsNativeResponses, sourceSupportsNativeProtocol } from "../../sourceProtocolBindings";
import { sourceHost } from "../../sourceUrl";
import { ActionMenu, ActionMenuItem, Button, EmptyState, ErrorDetailsDialog, IconButton, OptionMenu, StatusIcon, useConfirm } from "../../components/Ui";
import type { FeedbackError } from "../../state/feedback";
import { useRelayState } from "../../state/RelayStateProvider";
import { NoResults } from "./connectionHelpers";
import { routingOrderPositions, runtimeCandidateForMember, upcomingModelRetries } from "../../routingOrder";
import { updatePoolMembership } from "../../poolMembership";
import { useRelativeTimeClock } from "../../hooks/useRelativeTimeClock";
import { filterAndSortSources, type SourceSortKey, type SourceSortDirection } from "./sourceListModel";
import { sourceErrorDetails } from "./sourceError";

const EMPTY_SOURCES: SourceSummary[] = [];
const EMPTY_RUNTIME_ORDER: CandidateRuntimeSnapshot[] = [];

export function SourcesList({ query, onEdit, onRefresh, onRefreshSelected }: {
  query: string;
  onEdit: (source: SourceSummary) => void;
  onRefresh: (sourceId: string) => void;
  onRefreshSelected: (sourceIds: readonly string[]) => void;
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
  const localSource = mode !== "remote";
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
      () => localSource
        ? relayCommands.setSourceEnabled(source.id, enabled)
        : relayCommands.remoteAction({ type: "update_source", id: source.id }, { enabled }),
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
          if (localSource) await relayCommands.setSourceEnabled(id, enabled);
          else await relayCommands.remoteAction({ type: "update_source", id }, { enabled });
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
          if (localSource) await relayCommands.deleteSource(id);
          else await relayCommands.remoteAction({ type: "delete_source", id });
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
        <div className="sources-list-heading">
          <div className="sources-selection-heading">
            <input
              ref={selectAllRef}
              type="checkbox"
              checked={allVisibleSelected}
              disabled={!sources.length || Boolean(busy)}
              aria-label={t("sources.selectVisible")}
              onChange={(event) => setSelected((previous) => event.target.checked
                ? [...new Set([...previous, ...sources.map((source) => source.id)])]
                : previous.filter((id) => !sources.some((source) => source.id === id)))}
            />
            <span className="sources-list-count">{t("connections.sources")} <strong>{sources.length}</strong></span>
          </div>
          <div className="sources-sort-controls">
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
            ]}
          />
          {sort.key !== "runtime" ? (
            <IconButton
              label={t(sort.direction === "asc" ? "sources.sortDescending" : "sources.sortAscending", { column: t("sources.sortLabel") })}
              icon={sort.direction === "asc" ? <ArrowUp aria-hidden /> : <ArrowDown aria-hidden />}
              onClick={() => sortColumn(sort.key)}
            />
          ) : null}
          </div>
        </div>
        {selectedSources.length ? <div className="sources-selection-toolbar">
          <span>{t("sources.selectedCount", { count: selectedSources.length })}</span>
          <div className="sources-selection-actions">
            {mode !== "zenith" ? <>
              <Button variant="secondary" icon={<ListPlus aria-hidden />} disabled={Boolean(busy) || selectedSources.every((source) => pendingPool[source.id] ?? source.inPool)} onClick={() => void updateSelectedParticipation(true)}>{t("sources.addToPoolAction")}</Button>
              <Button variant="secondary" icon={<ListMinus aria-hidden />} disabled={Boolean(busy) || selectedSources.every((source) => !(pendingPool[source.id] ?? source.inPool))} onClick={() => void updateSelectedParticipation(false)}>{t("sources.removeFromPoolAction")}</Button>
            </> : null}
            <ActionMenu label={t("common.actions")}>
              <ActionMenuItem icon={<RefreshCw aria-hidden />} disabled={Boolean(busy)} onClick={() => onRefreshSelected(selectedSources.map((source) => source.id))}>{t("sources.refreshData")}</ActionMenuItem>
              <ActionMenuItem icon={<Power aria-hidden />} disabled={Boolean(busy) || selectedSources.every((source) => pendingEnabled[source.id] ?? source.enabled)} onClick={() => updateSelectedEnabled(true)}>{t("common.enable")}</ActionMenuItem>
              <ActionMenuItem icon={<Power aria-hidden />} disabled={Boolean(busy) || selectedSources.every((source) => !(pendingEnabled[source.id] ?? source.enabled))} onClick={() => updateSelectedEnabled(false)}>{t("common.disable")}</ActionMenuItem>
              <ActionMenuItem danger icon={<Trash2 aria-hidden />} disabled={Boolean(busy)} onClick={() => void deleteSelected()}>{t("common.delete")}</ActionMenuItem>
            </ActionMenu>
          </div>
          <IconButton label={t("accounts.clearSelection")} icon={<X aria-hidden />} disabled={Boolean(busy)} onClick={() => setSelected([])} />
        </div> : null}
        {!sources.length ? <NoResults /> : null}
        <div className="source-cards" role="list">{sources.map((source) => {
          const inPool = pendingPool[source.id] ?? source.inPool;
          const enabled = pendingEnabled[source.id] ?? source.enabled;
          const launchBusy = busy === `launch-source-${source.id}`;
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
             ? runtimeCandidateForMember(source.id, "api_source", runtimeOrder, "all", source.wireApi)
            : undefined;
          const runtimeTone = source.operationalStatus === "rotation" ? transientCandidateTone(runtimeState, nowMs, true) : null;
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
          const failure = sourceErrorDetails(source.lastErrorCode);
          const statusLabel = !enabled
            ? t("connections.status.disabled")
            : mode !== "zenith" && !inPool
              ? t("sources.notInPoolLabel")
              : source.operationalStatus === "rotation"
                ? t("sources.inPoolLabel")
                : t(`connections.status.${source.operationalStatus}`);
          const indicatorLabel = [runtimeHint, statusLabel].filter(Boolean).join(" · ");
          const indicatorTone = !enabled || (mode !== "zenith" && !inPool)
            ? "disabled"
            : runtimeTone ?? operationalStatusTone(source.operationalStatus);
          return <article className="source-card" role="listitem" key={source.id} data-source-id={source.id} data-enabled={enabled} data-selected={selected.includes(source.id)}>
            <header className="source-card-header">
              <label className="source-card-select">
                <input type="checkbox" checked={selected.includes(source.id)} disabled={Boolean(busy)}
                  aria-label={t("sources.select", { name: source.name })}
                  onChange={(event) => setSelected((previous) => event.target.checked
                    ? [...previous, source.id]
                    : previous.filter((id) => id !== source.id))} />
              </label>
              <div className="source-card-identity">
                <strong>{source.name}</strong>
                <span data-relay-tooltip={source.baseUrl}>{sourceHost(source.baseUrl)}</span>
              </div>
              <div className="row-actions">
                <ActionMenu>
                  <ActionMenuItem
                    icon={busy === `source-refresh-${source.id}` ? <Loader2 className="spin" aria-hidden /> : <RefreshCw aria-hidden />}
                    disabled={Boolean(busy)}
                    onClick={() => onRefresh(source.id)}
                  >
                    {t("sources.refreshData")}
                  </ActionMenuItem>
                  {mode !== "zenith" ? (
                    <ActionMenuItem
                      icon={inPool ? <ListMinus aria-hidden /> : <ListPlus aria-hidden />}
                      disabled={Boolean(busy)}
                      onClick={() => void updateParticipation(source, !inPool)}
                    >
                      {t(inPool ? "sources.removeFromPoolAction" : "sources.addToPoolAction")}
                    </ActionMenuItem>
                  ) : null}
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
                      () => localSource
                        ? relayCommands.deleteSource(source.id)
                        : relayCommands.remoteAction({ type: "delete_source", id: source.id }),
                      "feedback.deleted",
                      { backgroundRefresh: true },
                    ))}
                  >
                    {t("common.delete")}
                  </ActionMenuItem>
                </ActionMenu>
                <IconButton label={t("common.edit")} icon={<Pencil aria-hidden />} disabled={Boolean(busy)} onClick={() => onEdit(source)} />
                <IconButton
                  label={t("sources.launch")}
                  icon={<Play aria-hidden />}
                  busy={launchBusy}
                  disabled={launchDisabled}
                  title={launchTitle}
                  onClick={() => setLaunchSourceId(source.id)}
                />
              </div>
            </header>
            <div className="source-card-meta">
              <div className="source-card-status" data-tone={indicatorTone}>
                <StatusIcon status={indicatorTone} label={indicatorLabel} />
                <span>{statusLabel}</span>
              </div>
              <span className="source-card-models">{t("sources.groupModelsCount", { count: source.models.length })}</span>
            </div>
            {failure || runtimeHint ? <div className="source-card-diagnostics">
              {failure ? <button type="button" className="source-card-error" aria-label={`${t("feedback.errorDetails")}: ${failure.summary}`} onClick={() => setErrorDetails(failure.error)}>
                <CircleAlert aria-hidden /><span>{failure.summary}</span>
              </button> : null}
              {runtimeHint ? <small>{runtimeHint}</small> : null}
            </div> : null}
          </article>;
        })}</div>
      {errorDetails ? <ErrorDetailsDialog error={errorDetails} message={errorDetails.message} onClose={() => setErrorDetails(null)} /> : null}
      {launchSource ? <ApplicationPickerDialog
        title={t("sources.launchPickerTitle")}
        showLaunchToggle={false}
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
