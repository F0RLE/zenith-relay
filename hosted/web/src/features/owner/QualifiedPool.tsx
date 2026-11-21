import { useRef, useState } from "react";
import type { readOwnerOverview } from "../../api/ownerSession";
import { Plus } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import { inventoryForPool } from "./livePool";
import { LiveMemberEditor } from "./LiveMemberEditor";
import { LivePoolRotation } from "./LivePoolRotation";
import { OwnerSectionHeader } from "./OwnerSectionHeader";
import { PoolMembers } from "./PoolMembers";
import { PoolModelPicker } from "./PoolModelPicker";
import { PoolModelRules } from "./PoolModelRules";
import { useSourceServing } from "./SourceServing";
import { useOwnerRules } from "./useOwnerRules";
import "./live-pool.css";

type Session = Awaited<ReturnType<typeof readOwnerOverview>>;
const tabs = [["members", "pool.members"], ["rules", "modelRules.title"]] as const;

export function QualifiedPool({ session }: { session: Session }) {
  const { t } = useLocale();
  const root = useRef<HTMLDivElement>(null);
  const { rows, apply, blocked, busy, clearError, feedback, refresh } = useOwnerRules(session.csrf);
  const [tab, setTab] = useState<typeof tabs[number][0]>("members");
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState<string>();
  const [rotationOpen, setRotationOpen] = useState(false);
  const [rotationRevision, setRotationRevision] = useState(0);
  const { members, candidates } = inventoryForPool(rows);
  const member = members.find(item => item.id === editing);
  const focusTab = () => root.current?.querySelector<HTMLButtonElement>(".relay-tabs button[aria-pressed=true]")?.focus();
  const lifecycle = useSourceServing({
    sources: members, csrf: session.csrf, stale: blocked || !rows, focusTab,
    refresh: () => {
      if (document.activeElement?.closest(".live-pool-card")) focusTab();
      refresh();
    },
  });
  return <div ref={root} className="live-pool">
    <OwnerSectionHeader title="nav.pool" group="pool.poolSection" tabs={tabs} tab={tab} select={next => {
      if (busy) return;
      setTab(next); clearError();
    }} action={tab === "members" && rows && session.mode === "qualification" &&
      <button className="relay-button primary" type="button" aria-haspopup="dialog" disabled={lifecycle.blocked}
        onClick={() => setAdding(true)}><Plus aria-hidden="true" />{t("pool.addMember")}</button>} />
    {feedback}
    {lifecycle.result}
    {tab === "members" && rows && <PoolMembers members={members} onEdit={setEditing} rotationRevision={rotationRevision}
      onRotate={session.mode === "qualification" && !lifecycle.blocked ? () => setRotationOpen(true) : undefined} />}
    {tab === "rules" && rows && <PoolModelRules rows={rows} csrf={session.csrf} apply={apply}
      blocked={lifecycle.blocked} editable={session.mode === "qualification"} />}
    {adding && <PoolModelPicker candidates={candidates} blocked={lifecycle.blocked} close={() => setAdding(false)}
      admit={rule => { setAdding(false); void apply(rule, "admit"); }} />}
    {member && <LiveMemberEditor key={member.id} member={member} csrf={session.csrf}
      blocked={lifecycle.blocked || lifecycle.waiting(member)} close={() => setEditing(undefined)}
      saved={() => { setEditing(undefined); refresh(); }}
      change={action => { setEditing(undefined); lifecycle.select(member, action); }} />}
    {rotationOpen && <LivePoolRotation csrf={session.csrf} close={() => setRotationOpen(false)}
      saved={() => { setRotationOpen(false); setRotationRevision(n => n + 1); }} />}
    {lifecycle.dialog}
  </div>;
}
