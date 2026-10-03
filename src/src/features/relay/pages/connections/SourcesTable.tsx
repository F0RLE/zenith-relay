import { useEffect, useMemo, useState } from "react";
import { ArrowDown, ArrowUp, ArrowUpDown, ListMinus, ListPlus, Loader2, Pencil, Play, Power, RefreshCw, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { CandidateRuntimeSnapshot, SourceSummary } from "../../api/types";
import { operationalStatusTone, transientCandidateTone } from "../../accountStatus";
import { ApplicationPickerDialog } from "../../components/ApplicationPickerDialog";
import { formatDetailedRemainingTime } from "../../quotaFormatting";
import { sourceSupportsNativeResponses, sourceSupportsNativeProtocol } from "../../sourceProtocolBindings";
import { sourceHost } from "../../sourceUrl";
import { ActionMenu, ActionMenuItem, EmptyState, IconButton, OptionMenu, StatusIcon, useConfirm } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { NoResults } from "./connectionHelpers";
import { routingOrderPositions, runtimeCandidateForMember, upcomingModelRetries } from "../../routingOrder";
import { updatePoolMembership } from "../../poolMembership";
import { useRelativeTimeClock } from "../../hooks/useRelativeTimeClock";
import { filterAndSortSources, type SourceSortKey } from "./sourceTableModel";

type SourceSortDirection = "asc" | "desc";
const EMPTY_SOURCES: SourceSummary[] = [];
const EMPTY_RUNTIME_ORDER: CandidateRuntimeSnapshot[] = [];

export function SourcesTable({ query, onEdit, onRefresh }: { query: string; onEdit: (source: SourceSummary) => void; onRefresh: (sourceId: string) => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, activateCodexProfile, busy } = useRelayState();
  const confirm = useConfirm();
  const [sort, setSort] = useState<{ key: SourceSortKey; direction: SourceSortDirection }>({ key: "runtime", direction: "asc" });
  const [pendingPool, setPendingPool] = useState<Record<string, boolean>>({});
  const [pendingEnabled, setPendingEnabled] = useState<Record<string, boolean>>({});
  const [launchSourceId, setLaunchSourceId] = useState<string | null>(null);
  const sourcesSnapshot = runtime?.sources ?? EMPTY_SOURCES;
  const membershipSignature = sourcesSnapshot.map((source) => `${source.id}:${source.inPool}:${source.enabled}`).join("|");
  useEffect(() => {
    const savedPool = new Map(sourcesSnapshot.map((source) => [source.id, source.inPool]));
    const savedEnabled = new Map(sourcesSnapshot.map((source) => [source.id, source.enabled]));
    setPendingPool((current) => dropConfirmedFlags(current, savedPool));
    setPendingEnabled((current) => dropConfirmedFlags(current, savedEnabled));
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
  if (!runtime?.sources.length) {
    return <EmptyState title={t("sources.emptyTitle")} description={t("sources.emptyDescription")} />;
  }
  if (!sources.length) return <NoResults />;
  const localSource = mode !== "remote";
  const launchSource = launchSourceId ? sourcesSnapshot.find((source) => source.id === launchSourceId) ?? null : null;
  const sortColumn = (key: SourceSortKey) => setSort((current) =>
    current.key === key
      ? { key, direction: current.direction === "asc" ? "desc" : "asc" }
      : { key, direction: "asc" },
  );
  const sortLabel = (key: SourceSortKey, label: string) => {
    const active = sort.key === key;
    const direction = active ? sort.direction : "asc";
    const Icon = active ? (direction === "asc" ? ArrowUp : ArrowDown) : ArrowUpDown;
    return (
      <button
        className="source-sort-button"
        type="button"
        aria-label={t(direction === "asc" ? "sources.sortAscending" : "sources.sortDescending", { column: label })}
        aria-sort={active ? (direction === "asc" ? "ascending" : "descending") : "none"}
        onClick={() => sortColumn(key)}
      >
        <span>{label}</span><Icon aria-hidden />
      </button>
    );
  };
  const rememberFlag = (
    setFlag: (update: (current: Record<string, boolean>) => Record<string, boolean>) => void,
    id: string,
    value: boolean,
  ) => {
    setFlag((current) => ({ ...current, [id]: value }));
  };
  const rollbackFlag = (
    setFlag: (update: (current: Record<string, boolean>) => Record<string, boolean>) => void,
    id: string,
    value: boolean,
  ) => {
    setFlag((current) => {
      if (current[id] !== value) return current;
      const next = { ...current };
      delete next[id];
      return next;
    });
  };
  const updateParticipation = (source: SourceSummary, inPool: boolean) => {
    rememberFlag(setPendingPool, source.id, inPool);
    void perform(
      `source-pool-${source.id}`,
      () => updatePoolMembership(mode, { accountIds: [], sourceIds: [source.id], inPool }),
      "feedback.saved",
      { backgroundRefresh: true, uiLock: false },
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
      { backgroundRefresh: true, uiLock: false },
    ).then((ok) => {
      if (!ok) rollbackFlag(setPendingEnabled, source.id, enabled);
    });
  };
  return (
    <div className="relay-table-wrap connection-list-wrap relay-compact-content">
      <table className="relay-table source-table connection-table">
        <caption className="connection-mobile-sort">
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
        </caption>
        <thead><tr>
          <th aria-sort={sort.key === "status" ? (sort.direction === "asc" ? "ascending" : "descending") : "none"}>{sortLabel("status", t("common.status"))}</th>
          <th aria-sort={sort.key === "name" ? (sort.direction === "asc" ? "ascending" : "descending") : "none"}>{sortLabel("name", t("common.name"))}</th>
          <th aria-sort={sort.key === "server" ? (sort.direction === "asc" ? "ascending" : "descending") : "none"}>{sortLabel("server", t("sources.host"))}</th>
          <th aria-sort={sort.key === "models" ? (sort.direction === "asc" ? "ascending" : "descending") : "none"}>{sortLabel("models", t("common.models"))}</th>
          <th><span className="sr-only">{t("common.actions")}</span></th>
        </tr></thead>
        <tbody>{sources.map((source) => {
          const inPool = pendingPool[source.id] ?? source.inPool;
          const enabled = pendingEnabled[source.id] ?? source.enabled;
          const launchBusy = busy === `launch-source-${source.id}`;
          const supportsNative = sourceSupportsNativeProtocol(source);
          const launchDisabled = !localSource || !supportsNative || !enabled || !source.secretAvailable || launchBusy;
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
          const runtimeError = source.lastErrorCode?.trim()
            ? t("pool.runtimeError", { code: source.lastErrorCode.trim() })
            : null;
          const statusLabel = t(`connections.status.${source.operationalStatus}`);
          const indicatorLabel = [runtimeError, runtimeHint, statusLabel].filter(Boolean).join(" · ");
          const indicatorTone = runtimeError
            ? "error"
            : source.operationalStatus === "unavailable" || source.operationalStatus === "disabled"
            ? operationalStatusTone(source.operationalStatus)
            : runtimeTone ?? operationalStatusTone(source.operationalStatus);
          return <tr key={source.id} data-source-id={source.id}>
            <td><div className="connection-status"><StatusIcon status={indicatorTone} label={indicatorLabel} /><span>{statusLabel}</span></div></td>
            <td><div className="connection-identity"><strong>{source.name}</strong>{mode !== "zenith" ? <small>{t(inPool ? "sources.inPoolLabel" : "sources.notInPoolLabel")}</small> : null}</div></td>
            <td><code className="connection-host" data-relay-tooltip={source.baseUrl}>{sourceHost(source.baseUrl)}</code></td>
            <td><span className="connection-model-count">{source.models.length}</span></td>
            <td className="row-actions-cell">
              <div className="row-actions">
                <ActionMenu>
                  <ActionMenuItem
                    icon={busy === `source-refresh-${source.id}` ? <Loader2 className="spin" aria-hidden /> : <RefreshCw aria-hidden />}
                    disabled={busy === `source-refresh-${source.id}`}
                    onClick={() => onRefresh(source.id)}
                  >
                    {t("sources.refreshData")}
                  </ActionMenuItem>
                  {mode !== "zenith" ? (
                    <ActionMenuItem
                      icon={inPool ? <ListMinus aria-hidden /> : <ListPlus aria-hidden />}
                      onClick={() => void updateParticipation(source, !inPool)}
                    >
                      {t(inPool ? "sources.removeFromPoolAction" : "sources.addToPoolAction")}
                    </ActionMenuItem>
                  ) : null}
                  <ActionMenuItem
                    icon={<Power aria-hidden />}
                    onClick={() => void updateEnabled(source, !enabled)}
                  >
                    {enabled ? t("common.disable") : t("common.enable")}
                  </ActionMenuItem>
                  <ActionMenuItem
                    danger
                    icon={<Trash2 aria-hidden />}
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
                <IconButton label={t("common.edit")} icon={<Pencil aria-hidden />} onClick={() => onEdit(source)} />
                <IconButton
                  label={t("sources.launch")}
                  icon={<Play aria-hidden />}
                  busy={launchBusy}
                  disabled={launchDisabled}
                  title={launchTitle}
                  onClick={() => setLaunchSourceId(source.id)}
                />
              </div>
            </td>
          </tr>;
        })}</tbody>
      </table>
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
  const next = { ...pending };
  for (const [id, value] of Object.entries(pending)) {
    if (saved.get(id) === value) {
      delete next[id];
      changed = true;
    }
  }
  return changed ? next : pending;
}
