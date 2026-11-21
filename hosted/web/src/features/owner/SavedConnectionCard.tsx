import type { SavedSource } from "../../api/connections";
import { ListMinus, Pause, Pencil, Play, RefreshCw, RotateCcw } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import { ResourceCardHeading, ResourceMenu } from "./OwnerResourceCard";
import { SourceServingStatus, servingLabels, type ServingAction } from "./SourceServing";

export function SavedConnectionCard({ source, blocked, refreshOnly, webChecks, refresh, change, models, purchase }: {
  source: SavedSource; blocked: boolean; refreshOnly: boolean; webChecks: boolean; refresh: () => void;
  change: (action: ServingAction) => void; models: () => void; purchase: () => void;
}) {
  const { t, locale } = useLocale();
  const { serving } = source;
  const trashed = serving?.state === "trashed";
  const revoked = source.credential_state === "revoked";
  const paused = serving?.state === "paused";
  const name = source.label ?? source.id;
  const proxy = t(source.proxy_bound ? "connections.proxyAssigned" : "connections.noProxyAssigned");
  const state = t(revoked ? "connections.credentialRevoked" : trashed ? "connections.inTrash"
    : serving?.draining ? "sourceServing.draining" : paused ? "connections.paused"
    : serving?.in_pool ? "connections.inPool" : "connections.outsidePool");
  const primary = trashed ? "restore" : paused ? "resume" : "pause";
  const primaryLabel = t(refreshOnly ? "sourceServing.refresh" : servingLabels[primary]);
  const count = source.verified_models.length;
  const declared = (source.declared_models?.length ?? 0) > 0;
  const hasModels = count > 0 || serving?.in_pool || declared || webChecks && !!source.endpoint;
  return <article className="owner-resource-card saved-source-card" aria-label={name} data-source-id={source.id}
    data-kind="api" data-status={revoked ? "error" : trashed || paused ? "disabled" : serving?.in_pool ? "pool" : "idle"}>
    <ResourceCardHeading name={name} api status={state} meta={<>
      {source.endpoint && <span className="owner-source-endpoint" title={source.endpoint}>{source.endpoint}</span>}
      <span>{proxy}</span>
    </>}
      actions={serving && <ResourceMenu name={name} active actions={[{
        label: t(trashed ? "sourceServing.remove" : "connections.moveToTrash"), danger: true,
        disabled: blocked || (trashed && refreshOnly), run: () => change(trashed ? "remove" : "trash"),
      }]} />} />
    <div className="owner-source-body">
      <div className="saved-source-state">{serving ? <SourceServingStatus view={serving} /> : <span>{state}</span>}</div>
      {!trashed && !revoked && (hasModels ? <button type="button" className="owner-source-models" aria-haspopup="dialog" onClick={models}>
        <span>{t(count ? "connections.verifiedModels" : serving?.in_pool || !declared ? "source.section.models" : "sourceIntake.unverified")}</span>
        {count > 0 && <strong>{count.toLocaleString(locale)}</strong>}
      </button> : <div className="owner-source-models"><span>{t("connections.noVerifiedModels")}</span></div>)}
    </div>
    <footer className="owner-resource-footer">
      {serving?.in_pool && <button className="relay-icon-button owner-membership-action" type="button" aria-pressed="true"
        aria-disabled={blocked} aria-haspopup="dialog" aria-label={t(servingLabels.park)}
        data-relay-tooltip={t(servingLabels.park)} onClick={() => change("park")}><ListMinus aria-hidden="true" /></button>}
      {!trashed && <button className="relay-icon-button" type="button" aria-haspopup="dialog"
        aria-label={t("purchaseLimit.title")} data-relay-tooltip={t("purchaseLimit.title")} onClick={purchase}><Pencil aria-hidden="true" /></button>}
      {serving && <button className="relay-icon-button" type="button" aria-label={primaryLabel}
        aria-haspopup={trashed && !refreshOnly ? "dialog" : undefined} data-relay-tooltip={primaryLabel}
        aria-disabled={blocked && !refreshOnly} onClick={refreshOnly ? refresh : () => change(primary)}>
        {refreshOnly ? <RefreshCw aria-hidden="true" /> : trashed ? <RotateCcw aria-hidden="true" />
          : paused ? <Play aria-hidden="true" /> : <Pause aria-hidden="true" />}
      </button>}
    </footer>
  </article>;
}
