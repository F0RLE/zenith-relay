import { useId } from "react";
import { Pause, Pencil, Play, SlidersHorizontal, Users } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import { liveMemberOrder, livePoolActivity, type PoolMember } from "./livePool";
import { OwnerPoolRuntime } from "./OwnerPoolRuntime";
import { ResourceCardHeading } from "./OwnerResourceCard";
import { ResourceCounts } from "./OwnerResourceSummary";
import { SourceServingStatus } from "./SourceServing";
import { usePoolActivity } from "./usePoolActivity";

function MemberCard({ member, requests, onEdit }: {
  member: PoolMember; requests: number | undefined; onEdit: () => void;
}) {
  const { t, locale } = useLocale();
  const activityId = useId();
  const { serving, label, proxy, models } = member;
  return <article className="owner-resource-card saved-source-card live-pool-card" aria-label={label}
    aria-describedby={requests ? activityId : undefined} data-source-id={member.id} data-kind="api"
    data-status={serving.state === "enabled" ? "pool" : "disabled"}
    data-active-requests={requests} data-current={requests === undefined ? undefined : requests > 0}>
    {requests ? <span id={activityId} className="sr-only">{t("pool.activity.requestsUnknown", { count: requests })}</span> : null}
    <ResourceCardHeading name={label} api actions={null} status={t("connections.apiProvider")}
      meta={<span>{t(proxy ? "connections.proxyAssigned" : "connections.noProxyAssigned")}</span>} />
    <div className="owner-source-body">
      <div className="saved-source-state"><SourceServingStatus view={serving} /></div>
      <div className="owner-source-models"><span>{t("pool.memberModels")}</span><strong>{models.length.toLocaleString(locale)}</strong></div>
    </div>
    <footer className="owner-resource-footer">
      <button type="button" className="relay-icon-button" aria-label={t("pool.editMemberNamed", { name: label })}
        aria-haspopup="dialog" data-relay-tooltip={t("common.edit")} onClick={onEdit}><Pencil aria-hidden="true" /></button>
    </footer>
  </article>;
}

export function PoolMembers({ members, onEdit, onRotate, rotationRevision }: {
  members: PoolMember[]; onEdit: (id: string) => void; onRotate?: (() => void) | undefined; rotationRevision: number;
}) {
  const { t } = useLocale();
  const view = usePoolActivity(rotationRevision);
  const activity = livePoolActivity(view, members);
  const ordered = liveMemberOrder(members, view);
  const enabled = members.filter(member => member.serving.state === "enabled").length;
  return <>
    <section className="owner-resource-controls live-pool-summary" aria-label={t("pool.members")}>
      <div className="owner-resource-summary-band"><ResourceCounts stats={[
        { label: "pool.memberCount", value: members.length, icon: <Users aria-hidden="true" /> },
        { label: "pool.enabled", value: enabled, icon: <Play aria-hidden="true" /> },
        { label: "connections.paused", value: members.length - enabled, icon: <Pause aria-hidden="true" /> },
      ]} /></div>
      {view?.strategy !== null && <div className="owner-resource-toolbar owner-pool-toolbar live-pool-toolbar">
        <OwnerPoolRuntime activity={activity} mode="automatic"
          strategy={view?.strategy ? t(`pool.rotation.modes.${view.strategy}`) : undefined} />
        {onRotate && <button className="relay-icon-button live-rotation-trigger" type="button" aria-haspopup="dialog"
          aria-label={t("pool.routingSettings")} data-relay-tooltip={t("pool.routingSettings")} onClick={onRotate}>
          <SlidersHorizontal aria-hidden="true" />
        </button>}
      </div>}
    </section>
    {members.length ? <div className="owner-resource-grid">{ordered.map(member =>
      <MemberCard key={member.id} member={member} requests={view ? activity.requests.get(member.id) ?? 0 : undefined}
        onEdit={() => onEdit(member.id)} />)}</div>
      : <p className="owner-resource-empty" role="status">{t("pool.noAdmittedMembers")}</p>}
  </>;
}
