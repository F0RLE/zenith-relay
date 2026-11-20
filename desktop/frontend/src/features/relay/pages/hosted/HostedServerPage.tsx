import { useEffect, useRef, useState } from "react";
import { ExternalLink } from "lucide-react";
import { useTranslation } from "react-i18next";
import { forgetPairing, getPairing, openDemo, preparePairing, readConnections } from "../../api/hostedDevice";
import { Button, CopyButton, PageHeader } from "../../components/Ui";
import type { HostedDevicePage, HostedDeviceReview } from "../../api/types/hostedDevice";
import { HostedSourceCopy } from "./HostedSourceCopy";
import "./hosted-server.css";

export function HostedServerPage() {
  const { t, i18n } = useTranslation();
  const [device,setDevice]=useState<HostedDeviceReview|null>(),[invitation,setInvitation]=useState("");
  const [page,setPage]=useState<HostedDevicePage>(),[cursors,setCursors]=useState<string[]>([]);
  const [error,setError]=useState(""),[busy,setBusy]=useState(false),[confirm,setConfirm]=useState(false);
  const [restore,setRestore]=useState(0);
  const running=useRef(false);
  const [opening, setOpening] = useState(false);
  const [failed, setFailed] = useState(false);
  useEffect(()=>{
    let mounted=true;
    void getPairing().then(d=>{if(mounted){setDevice(d);setError("");}})
      .catch(()=>{if(mounted)setError("storage");});
    return ()=>{mounted=false;};
  },[restore]);
  // A page is a snapshot. Expire it locally without polling or extending a grant.
  useEffect(() => {
    if (!page) return;
    const timer = window.setTimeout(() => {
      setPage(undefined); setCursors([]); setError("denied");
    }, Math.min(2_147_483_647, Math.max(0, page.expires_at_s * 1000 - Date.now())));
    return () => window.clearTimeout(timer);
  }, [page]);
  async function act(action:"prepare"|"forget"|"read",next=cursors) {
    if(running.current)return;
    running.current=true;setBusy(true);setError("");setConfirm(false);
    try{
      if(action==="prepare") {setDevice(await preparePairing(invitation.trim()));setInvitation("");}
      else if(device && action==="forget"){await forgetPairing(device.invitation);setDevice(null);setPage(undefined);setCursors([]);}
      else if(device){const v=await readConnections(device.invitation,next.at(-1));setPage(v);setCursors(next);}
    }catch(e){
      setPage(undefined);setCursors([]);
      if (e === "device_target_changed") {
        setDevice(undefined); setRestore(n => n + 1);
      } else {
        setError(e==="copy_review_pending"?"pendingCopy":e==="device_not_authorized"?"denied":e==="invalid_device_invitation"?"invalid":e==="device_storage_unavailable"?"storage":"unavailable");
      }
    }finally{running.current=false;setBusy(false);}
  }
  async function openPreview() {
    if (opening) return;
    setOpening(true); setFailed(false);
    try { await openDemo(); }
    catch { setFailed(true); }
    finally { setOpening(false); }
  }
  return <section className="relay-page">
    <PageHeader title={t("modes.hosted")} />
    <div className="hosted-device-pane">
      {device===undefined && !error && <p role="status">{t("common.loading")}</p>}
      {device===null && <form onSubmit={e=>{e.preventDefault();if(invitation.trim())void act("prepare");}}>
        <label className="relay-field"><span>{t("hosted.invitation")}</span>
          <input value={invitation} onChange={e=>setInvitation(e.target.value)} maxLength={512} autoComplete="off" spellCheck={false} /></label>
        <Button type="submit" variant="primary" disabled={busy||!invitation.trim()}>{t("hosted.prepare")}</Button>
      </form>}
      {device && <>
        <div className="hosted-device-target"><strong>{device.destination.origin}</strong><code>{device.destination.installation}</code>
          <span>{t("hosted.scope")}</span></div>
        {!page && <label className="relay-field"><span>{t("hosted.review")}</span>
          <input readOnly value={device.approval_code} onFocus={e=>e.target.select()} />
        </label>}
        <div className="hosted-device-actions">
          {!page && <CopyButton value={device.approval_code} label={t("common.copy")} />}
          <Button variant="primary" disabled={busy} onClick={()=>void act("read")}>{t(page?"common.refresh":"hosted.read")}</Button>
          <Button disabled={busy} onClick={()=>confirm?void act("forget"):setConfirm(true)}>{t(confirm?"hosted.confirmForget":"hosted.forget")}</Button>
          {confirm && <Button disabled={busy} onClick={()=>setConfirm(false)}>{t("common.cancel")}</Button>}
        </div>
        {page && <section aria-label={t("hosted.read")}><small>{t("hosted.expires",{date:new Date(page.expires_at_s*1000).toLocaleString(i18n.resolvedLanguage, { dateStyle: "medium", timeStyle: "short" })})}</small>
          {page.items.length===0?<p>{t("hosted.empty")}</p>:<ul className="hosted-source-list">{page.items.map(v=><li key={v.id}>
            <strong>{v.label??v.id}</strong><span>{t(v.proxy_bound?"hosted.withProxy":"hosted.noProxy")}</span>
            <small>{v.credential_state==="revoked"?t("hosted.revoked"):v.verified_models.join(", ")||t("hosted.unchecked")}</small>
          </li>)}</ul>}
          <div className="hosted-device-actions">{cursors.length>0&&<Button disabled={busy} onClick={()=>void act("read",cursors.slice(0,-1))}>{t("common.back")}</Button>}
            {page.next_after&&<Button disabled={busy} onClick={()=>void act("read",[...cursors,page.next_after!])}>{t("hosted.next")}</Button>}</div>
        </section>}
        {device.destination.origin.startsWith("https://") && <HostedSourceCopy key={device.invitation} invitation={device.invitation} />}
      </>}
      {error && <p role="alert">{t(error==="pendingCopy"?"hostedCopy.pending":error==="invalid"?"hosted.invalid":error==="storage"?"hosted.storage":error==="denied"?"hosted.denied":"hosted.unavailable")}</p>}
      {device===undefined && error && <Button onClick={()=>setRestore(n=>n+1)}>{t("common.retry")}</Button>}
      {import.meta.env.DEV && <>
        <Button variant="ghost" icon={<ExternalLink aria-hidden />} disabled={opening} onClick={() => { void openPreview(); }}>{t("hosted.openPreview")}</Button>
        {failed && <p role="alert">{t("hosted.previewUnavailable")} <code>cd hosted/web; bun run dev</code></p>}
      </>}
    </div>
  </section>;
}
