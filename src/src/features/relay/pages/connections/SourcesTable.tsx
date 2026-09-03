import { useMemo, useState } from "react";
import { ArrowDown, ArrowUp, ArrowUpDown, ListMinus, ListPlus, Loader2, Pencil, Play, Power, RefreshCw, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { CandidateRuntimeSnapshot, SourceSummary } from "../../api/types";
import { operationalStatusTone, transientCandidateTone } from "../../accountStatus";
import { SourceProtocolBindingsSummary } from "../../components/SourceProtocolBindingsEditor";
import { formatDetailedRemainingTime } from "../../quotaFormatting";
import { effectiveSourceProtocolBindings, sourceSupportsAnyWireApi, sourceSupportsNativeResponses } from "../../sourceProtocolBindings";
import { sourceHost } from "../../sourceUrl";
import { ActionMenu, ActionMenuItem, EmptyState, IconButton, StatusIcon, useConfirm } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { NoResults, matchesQuery } from "./connectionHelpers";
import { compareRoutingOrder, routingOrderPositions, runtimeCandidateForMember, upcomingModelRetries } from "../../routingOrder";
import { compareStableText } from "../../poolHelpers";
import { updatePoolMembership } from "../../poolMembership";
import { useRelativeTimeClock } from "../../hooks/useRelativeTimeClock";

type SourceSortColumn = "status" | "name" | "server" | "route" | "models";
type SourceSortKey = "runtime" | SourceSortColumn;
type SourceSortDirection = "asc" | "desc";

const sourceStatusRank: Record<SourceSummary["operationalStatus"], number> = {
  disabled: 0,
  unavailable: 1,
  quotaWait: 2,
  rotation: 3,
};
const EMPTY_SOURCES: SourceSummary[] = [];
const EMPTY_RUNTIME_ORDER: CandidateRuntimeSnapshot[] = [];

function sourceSortValue(source: SourceSummary, key: SourceSortColumn) {
  switch (key) {
    case "status": return sourceStatusRank[source.operationalStatus];
    case "server": return sourceHost(source.baseUrl);
    case "route": return effectiveSourceProtocolBindings(source)
      .map((binding) => `${binding.wireApi}:${binding.adapter ?? "native"}`)
      .join(",");
    case "models": return source.models.length;
    case "name": return source.name;
  }
}

function compareSourcesForTable(
  left: SourceSummary,
  right: SourceSummary,
  key: SourceSortKey,
  direction: SourceSortDirection,
  runtimePosition: ReadonlyMap<string, number>,
) {
  if (key === "runtime") {
    return compareRoutingOrder(left.id, right.id, runtimePosition)
      || compareStableText(left.name, right.name)
      || compareStableText(left.id, right.id);
  }
  const leftValue = sourceSortValue(left, key);
  const rightValue = sourceSortValue(right, key);
  const primary = typeof leftValue === "number" && typeof rightValue === "number"
    ? leftValue - rightValue
    : compareStableText(String(leftValue), String(rightValue));
  if (primary) return direction === "asc" ? primary : -primary;
  return compareStableText(left.name, right.name) || compareStableText(left.id, right.id);
}

export function SourcesTable({ query, onEdit, onRefresh }: { query: string; onEdit: (source: SourceSummary) => void; onRefresh: (sourceId: string) => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, activateCodexProfile, busy } = useRelayState();
  const confirm = useConfirm();
  const [sort, setSort] = useState<{ key: SourceSortKey; direction: SourceSortDirection }>({ key: "runtime", direction: "asc" });
  const sourcesSnapshot = runtime?.sources ?? EMPTY_SOURCES;
  const runtimeOrder = runtime?.gateway.routingOrder ?? EMPTY_RUNTIME_ORDER;
  const retryTimestamps = useMemo(() => runtimeOrder
    .flatMap((candidate) => candidate.kind === "api_source" ? [candidate.nextRetryAtMs] : []), [runtimeOrder]);
  const nowMs = useRelativeTimeClock(retryTimestamps);
  const runtimePosition = useMemo(() => routingOrderPositions(runtimeOrder), [runtimeOrder]);
  const sources = useMemo(() => sourcesSnapshot
    .filter((source) => matchesQuery(
      query,
      source.name,
      source.baseUrl,
      effectiveSourceProtocolBindings(source).map((binding) => binding.wireApi),
      source.models,
    ))
    .sort((left, right) => compareSourcesForTable(left, right, sort.key, sort.direction, runtimePosition)),
  [query, runtimePosition, sort.direction, sort.key, sourcesSnapshot]);
  if (!runtime?.sources.length) {
    return <EmptyState title={t("sources.emptyTitle")} description={t("sources.emptyDescription")} />;
  }
  if (!sources.length) return <NoResults />;
  const localSource = mode !== "remote";
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
  const updateParticipation = (source: SourceSummary, inPool: boolean) => perform(
    `source-pool-${source.id}`,
    () => updatePoolMembership(mode, { accountIds: [], sourceIds: [source.id], inPool }),
    "feedback.saved",
  );
  return (
    <div className="relay-table-wrap relay-compact-content">
      <table className="relay-table source-table">
        <thead><tr>
          <th aria-sort={sort.key === "status" ? (sort.direction === "asc" ? "ascending" : "descending") : "none"}>{sortLabel("status", t("common.status"))}</th>
          <th aria-sort={sort.key === "name" ? (sort.direction === "asc" ? "ascending" : "descending") : "none"}>{sortLabel("name", t("common.name"))}</th>
          <th aria-sort={sort.key === "server" ? (sort.direction === "asc" ? "ascending" : "descending") : "none"}>{sortLabel("server", t("sources.host"))}</th>
          <th aria-sort={sort.key === "route" ? (sort.direction === "asc" ? "ascending" : "descending") : "none"}>{sortLabel("route", t("sources.route"))}</th>
          <th aria-sort={sort.key === "models" ? (sort.direction === "asc" ? "ascending" : "descending") : "none"}>{sortLabel("models", t("common.models"))}</th>
          <th><span className="sr-only">{t("common.actions")}</span></th>
        </tr></thead>
        <tbody>{sources.map((source) => {
          const launchBusy = busy === `launch-source-${source.id}`;
          const supportsAnyRoute = sourceSupportsAnyWireApi(source);
          const supportsNativeResponses = sourceSupportsNativeResponses(source);
          const launchDisabled = !localSource || !supportsNativeResponses || !source.enabled || !source.secretAvailable || launchBusy;
          const launchTitle = !localSource
            ? t("sources.launchLocalOnly")
            : !supportsNativeResponses
              ? t("sources.launchResponsesOnly")
              : !source.enabled || !source.secretAvailable
                ? t("sources.launchUnavailable")
                : t("sources.launch");
          const runtimeState = source.inPool
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
          return <tr key={source.id}>
            <td><StatusIcon status={indicatorTone} label={indicatorLabel} /></td>
            <td><strong>{source.name}</strong></td>
            <td><code>{sourceHost(source.baseUrl)}</code></td>
            <td><SourceProtocolBindingsSummary source={source} /></td>
            <td>{source.models.length}</td>
            <td className="row-actions-cell"><div className="row-actions">
              <ActionMenu>
                <ActionMenuItem icon={busy === `source-refresh-${source.id}` ? <Loader2 className="spin" aria-hidden /> : <RefreshCw aria-hidden />} disabled={Boolean(busy)} onClick={() => onRefresh(source.id)}>{t("sources.refreshData")}</ActionMenuItem>
                {mode !== "zenith" ? <ActionMenuItem icon={source.inPool ? <ListMinus aria-hidden /> : <ListPlus aria-hidden />} disabled={busy === `source-pool-${source.id}` || (!source.inPool && !supportsAnyRoute)} title={!source.inPool && !supportsAnyRoute ? t("sources.poolResponsesOnly") : undefined} onClick={() => void updateParticipation(source, !source.inPool)}>{t(source.inPool ? "sources.removeFromPoolAction" : "sources.addToPoolAction")}</ActionMenuItem> : null}
                <ActionMenuItem icon={<Power aria-hidden />} onClick={() => perform(`toggle-${source.id}`, () => localSource ? relayCommands.setSourceEnabled(source.id, !source.enabled) : relayCommands.remoteAction({ type: "update_source", id: source.id }, { enabled: !source.enabled }), "feedback.saved")}>{source.enabled ? t("common.disable") : t("common.enable")}</ActionMenuItem>
                <ActionMenuItem danger icon={<Trash2 aria-hidden />} onClick={() => void confirm(t("sources.deleteConfirm"), { danger: true }).then((accepted) => accepted && perform(`delete-${source.id}`, () => localSource ? relayCommands.deleteSource(source.id) : relayCommands.remoteAction({ type: "delete_source", id: source.id }), "feedback.deleted"))}>{t("common.delete")}</ActionMenuItem>
              </ActionMenu>
              <IconButton label={t("common.edit")} icon={<Pencil aria-hidden />} onClick={() => onEdit(source)} />
              <IconButton label={t("sources.launch")} icon={launchBusy ? <Loader2 className="spin" aria-hidden /> : <Play aria-hidden />} disabled={launchDisabled} title={launchTitle} onClick={() => {
                void activateCodexProfile(`launch-source-${source.id}`, () => relayCommands.launchCodexSource(source.id), true)
                  .then((activated) => { if (activated) localStorage.setItem("relay.directSourceId", source.id); });
              }} />
            </div></td>
          </tr>;
        })}</tbody>
      </table>
    </div>
  );
}
