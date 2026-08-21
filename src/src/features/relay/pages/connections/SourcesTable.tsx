import { useEffect, useState } from "react";
import { ListMinus, ListPlus, Loader2, Pencil, Play, Power, RefreshCw, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { SourceSummary } from "../../api/types";
import { operationalStatusTone, transientCandidateTone } from "../../accountStatus";
import { SourceProtocolBindingsSummary } from "../../components/SourceProtocolRoutingDisclosure";
import { formatDetailedRemainingTime } from "../../quotaFormatting";
import { effectiveSourceProtocolBindings, sourceSupportsNativeResponses, sourceSupportsWireApi } from "../../sourceProtocolBindings";
import { ActionMenu, ActionMenuItem, EmptyState, IconButton, StatusIcon, useConfirm } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { NoResults, matchesQuery } from "./connectionHelpers";
import { runtimeCandidateForMember } from "../../routingOrder";
export function SourcesTable({ query, onEdit }: { query: string; onEdit: (source: SourceSummary) => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, activateCodexProfile, busy } = useRelayState();
  const confirm = useConfirm();
  const [nowMs, setNowMs] = useState(Date.now());
  const sourceCooldownDeadline = Math.min(...(runtime?.gateway.routingOrder ?? [])
    .flatMap((candidate) => candidate.kind === "api_source" && candidate.nextRetryAtMs != null && candidate.nextRetryAtMs > nowMs ? [candidate.nextRetryAtMs] : []));
  useEffect(() => {
    if (!Number.isFinite(sourceCooldownDeadline)) return;
    const timer = window.setTimeout(() => setNowMs(Date.now()), sourceCooldownDeadline - nowMs < 60 * 60_000 ? 1_000 : 60_000);
    return () => window.clearTimeout(timer);
  }, [nowMs, sourceCooldownDeadline]);
  if (!runtime?.sources.length) {
    return <EmptyState title={t("sources.emptyTitle")} description={t("sources.emptyDescription")} />;
  }
  const sources = runtime.sources.filter((source) => matchesQuery(
    query,
    source.name,
    source.baseUrl,
    effectiveSourceProtocolBindings(source).map((binding) => binding.wireApi),
    source.models,
  ));
  if (!sources.length) return <NoResults />;
  const localSource = mode !== "remote";
  const runtimeOrder = runtime.gateway.routingOrder ?? [];
  const updateParticipation = (source: SourceSummary, inPool: boolean) => perform(
    `source-pool-${source.id}`,
    () => localSource
      ? relayCommands.setPoolMembership([], [source.id], inPool)
      : relayCommands.remoteAction({ type: "set_pool_membership" }, { accountIds: [], sourceIds: [source.id], inPool }),
    "feedback.saved",
  );
  const refreshModels = (source: SourceSummary) => perform(
    `source-models-${source.id}`,
    () => localSource
      ? relayCommands.testSource(source.id)
      : relayCommands.remoteAction({ type: "test_source", id: source.id }),
    "feedback.refreshed",
  );
  return (
    <div className="relay-table-wrap relay-compact-content">
      <table className="relay-table source-table">
        <thead><tr><th>{t("common.status")}</th><th>{t("common.name")}</th><th>{t("sources.host")}</th><th>{t("sources.route")}</th><th>{t("common.models")}</th><th><span className="sr-only">{t("common.actions")}</span></th></tr></thead>
        <tbody>{sources.map((source) => {
          const launchBusy = busy === `launch-source-${source.id}`;
          const supportsResponses = sourceSupportsWireApi(source, "responses");
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
            ? runtimeCandidateForMember(source.id, "api_source", runtimeOrder, "responses", source.wireApi)
            : undefined;
          const runtimeTone = source.operationalStatus === "rotation" ? transientCandidateTone(runtimeState, nowMs, true) : null;
          const modelRetries = [...(runtimeState?.modelRetries ?? [])].filter((retry) => retry.retryAtMs > nowMs).sort((left, right) => left.retryAtMs - right.retryAtMs);
          const runtimeHint = runtimeState?.halfOpen
            ? t("pool.recoveryProbe")
            : modelRetries.length
              ? t("pool.modelRetryAt", {
                models: modelRetries.map((retry) => retry.model).join(", "),
                time: formatDetailedRemainingTime(modelRetries[0].retryAtMs, nowMs, t),
              })
            : runtimeState?.nextRetryAtMs != null && runtimeState.nextRetryAtMs > nowMs
              ? t("pool.retryAt", { time: formatDetailedRemainingTime(runtimeState.nextRetryAtMs, nowMs, t) })
              : null;
          const runtimeError = source.lastErrorCode?.trim()
            ? t("pool.runtimeError", { code: source.lastErrorCode.trim() })
            : null;
          const statusLabel = t(`connections.status.${source.operationalStatus}`);
          const indicatorLabel = [runtimeError, runtimeHint, statusLabel].filter(Boolean).join(" · ");
          const indicatorTone = source.operationalStatus === "unavailable" || source.operationalStatus === "disabled"
            ? operationalStatusTone(source.operationalStatus)
            : runtimeTone ?? operationalStatusTone(source.operationalStatus);
          return <tr key={source.id}>
            <td><StatusIcon status={indicatorTone} label={indicatorLabel} /></td>
            <td><strong>{source.name}</strong></td>
            <td><code>{safeHost(source.baseUrl)}</code></td>
            <td><SourceProtocolBindingsSummary source={source} /></td>
            <td>{source.models.length}</td>
            <td className="row-actions-cell"><div className="row-actions">
              <ActionMenu>
                <ActionMenuItem icon={busy === `source-models-${source.id}` ? <Loader2 className="spin" aria-hidden /> : <RefreshCw aria-hidden />} disabled={busy === `source-models-${source.id}`} onClick={() => void refreshModels(source)}>{t("sources.refreshModels")}</ActionMenuItem>
                {mode !== "zenith" ? <ActionMenuItem icon={source.inPool ? <ListMinus aria-hidden /> : <ListPlus aria-hidden />} disabled={busy === `source-pool-${source.id}` || (!source.inPool && !supportsResponses)} title={!source.inPool && !supportsResponses ? t("sources.poolResponsesOnly") : undefined} onClick={() => void updateParticipation(source, !source.inPool)}>{t(source.inPool ? "sources.removeFromPoolAction" : "sources.addToPoolAction")}</ActionMenuItem> : null}
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
function safeHost(value: string) {
  try { return new URL(value).host; } catch { return value; }
}
