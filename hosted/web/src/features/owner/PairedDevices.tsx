import { useEffect, useRef, useState } from "react";
import { changeDevice, readDevices, validDeviceApproval, validDeviceLabel, type ChangeDevice, type DeviceList, type PairedDevice } from "../../api/devices";
import { ControlError, observeControl } from "../../api/control";
import { readOwnerOverview, signInOwner } from "../../api/ownerSession";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";

type Session = Awaited<ReturnType<typeof readOwnerOverview>>;
type Props = { session:Session; onSession:(s:Session)=>void };
const labels = { pending:"device.pending", active:"status.active", expired:"account.expired", revoked:"status.revoked" } as const;

function Manager({session,onSession}:Props) {
  const {t,locale} = useLocale();
  const [list,setList]=useState<DeviceList>(),[retry,setRetry]=useState(0);
  const [error,setError]=useState<MessageKey>(),[busy,setBusy]=useState(false),[label,setLabel]=useState("");
  const [code,setCode]=useState(""),[selected,setSelected]=useState(""),[confirm,setConfirm]=useState("");
  const flight=useRef(false), epoch=useRef(0), last=useRef<{signature:string;op:string}|undefined>(undefined);
  useEffect(()=>observeControl(async()=>({n:epoch.current,v:await readDevices()}),({n,v})=>{
    if (!flight.current && n===epoch.current) {setList(v);setError(e=>e==="unavailable.server"?undefined:e);}
  },e=>{
    if(flight.current)return;
    if(e instanceof ControlError && e.status===401)location.replace("/admin/login");
    else {setList(undefined);setError("unavailable.server");}
  }),[retry]);
  useEffect(() => {
    // Refresh once at the next visible deadline, not on a keep-alive interval.
    const now = Date.now();
    const deadlines = list?.items.filter(r => r.state === "pending" || r.state === "active")
      .map(r => (r.expires_at_s + 1) * 1000 - now).filter(ms => ms > 0);
    if (!deadlines?.length) return;
    const timer = window.setTimeout(() => { if (!document.hidden) setRetry(n => n + 1); }, Math.min(...deadlines, 2_147_483_647));
    return () => window.clearTimeout(timer);
  }, [list]);

  async function apply(action:ChangeDevice["action"],row?:PairedDevice) {
    if(flight.current)return;
    flight.current=true; epoch.current++; setBusy(true);setError(undefined);
    const signature=JSON.stringify([action,row?.id,row?.revision,action==="invite"?label:action==="approve"?code:""]);
    if(last.current?.signature!==signature)last.current={signature,op:crypto.randomUUID()};
    const op=last.current.op;
    const cmd:ChangeDevice={action,operation_id:op,device_id:row?.id??op,expected_revision:row?.revision??"0",
      ...(action==="invite"?{label:label.trim()}:action==="approve"?{approval_code:code.trim()}:{})};
    try{
      try{await changeDevice(cmd,session.csrf);}catch(e){
        if(!(e instanceof ControlError)||e.code!=="fresh_passkey_required")throw e;
        await signInOwner();const next=await readOwnerOverview(AbortSignal.timeout(10000));onSession(next);
        await changeDevice(cmd,next.csrf);
      }
      setList(await readDevices());setSelected(action==="invite"?op:"");setCode("");setLabel("");setConfirm("");last.current=undefined;
    }catch(e){
      if(e instanceof ControlError && e.status===401)location.replace("/admin/login");
      else if(e instanceof ControlError && e.status===409){setError("device.changed");setRetry(n=>n+1);}
      else setError(e instanceof ControlError && e.status===429?"device.limit":"device.retry");
    }finally{flight.current=false;setBusy(false);}
  }
  const date=(n:number)=>new Intl.DateTimeFormat(locale,{dateStyle:"medium",timeStyle:"short",timeZone:"UTC"}).format(n*1000)+" UTC";
  return <div className="device-manager entry-catalog">
    {error && <p role="alert">{t(error)}</p>}
    {!list ? (error ? <button className="key-button key-secondary" onClick={()=>{setError(undefined);setRetry(n=>n+1);}}>{t("common.retry")}</button>
      : <p role="status">{t("unavailable.connecting")}</p>) : <>
      <form className="owner-setup-form" onSubmit={e=>{e.preventDefault();if(validDeviceLabel(label.trim()))void apply("invite");}}>
        <label>{t("device.label")}<input autoComplete="off" maxLength={64} value={label} disabled={busy} onChange={e=>setLabel(e.target.value)} /></label>
        <button className="key-button key-secondary" disabled={busy||!validDeviceLabel(label.trim())}>{t("device.invite")}</button>
      </form>
      {list.items.length>0 && <ul className="catalog-list">{list.items.map(row=><li key={row.id} className="catalog-editor">
        <div className="catalog-identity"><strong>{row.label}</strong><small>{t(labels[row.state])} · {t("device.scope")}</small>
          <small>{t("keys.validUntil")}: {date(row.expires_at_s)}</small>
          {row.last_used_at_s && <small>{t("device.lastUsed")}: {date(row.last_used_at_s)}</small>}</div>
        {row.state==="pending" && (selected===row.id ? <form className="owner-setup-form"
          onSubmit={e=>{e.preventDefault();if(validDeviceApproval(row.id, code.trim())) void apply("approve",row);}}>
          <label>{t("device.invitation")}<input readOnly value={`${location.origin}/admin/connections#relay-device=${list.installation_id}.${row.id}`} onFocus={e=>e.target.select()} /></label>
          <label>{t("device.review")}<input autoComplete="off" spellCheck={false} value={code} maxLength={110} onChange={e=>setCode(e.target.value)} /></label>
          <button className="key-button key-primary" disabled={busy||!validDeviceApproval(row.id, code.trim())}>{t("device.approve")}</button>
        </form> : <button className="key-button key-secondary" disabled={busy} onClick={()=>{setSelected(row.id);setCode("");}}>{t("device.reviewInvite")}</button>)}
        {(row.state==="active" || row.state==="pending") && <div className="catalog-actions">
          <button className="key-button key-secondary" disabled={busy} onClick={()=>confirm===row.id?void apply("revoke",row):setConfirm(row.id)}>
            {t(confirm===row.id?"device.confirmRevoke":row.state==="pending"?"common.cancel":"device.revoke")}</button>
          {confirm===row.id && <button className="key-button key-secondary" disabled={busy} onClick={()=>setConfirm("")}>{t("common.cancel")}</button>}
        </div>}
      </li>)}</ul>}
    </>}
  </div>;
}
export function PairedDevices(props:Props) {
  const {t}=useLocale();const [open,setOpen]=useState(false);
  return <details className="device-panel" onToggle={e=>setOpen(e.currentTarget.open)}>
    <summary>{t("device.title")}</summary>{open && <Manager {...props}/>}
  </details>;
}
