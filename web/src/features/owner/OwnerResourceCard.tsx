import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Cloud, ListMinus, ListPlus, MoreHorizontal, Pause, Pencil, Play, RefreshCw, RotateCcw, UserRound } from "../../components/Icons";
import type { CustomerModel } from "../models/ModelsPage";
import type { ConnectionAction, OwnerConnection } from "./model";
import { OwnerAccountBody } from "./OwnerAccountBody";
import { OwnerSourceBody } from "./OwnerSourceBody";
import { sourceEndpoint } from "./sourcePresentation";
import { ResourceSelection } from "./ResourceSelection";
import { useLocale } from "../../i18n/LocaleProvider";

export function ResourceMenu({ name, active, actions }: { name: string; active: boolean; actions: { label: string; run: () => void; disabled?: boolean; danger?: boolean }[] }) {
  const { t } = useLocale();
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const id = useId();
  function close(restore = false) { setOpen(false); if (restore) trigger.current?.focus(); }
  useEffect(() => { if (!active) setOpen(false); }, [active]);
  useLayoutEffect(() => {
    if (!open) return;
    function place() {
      const anchor = trigger.current?.getBoundingClientRect(), menu = panel.current;
      if (!anchor || !menu || !trigger.current?.getClientRects().length) { setOpen(false); return; }
      const { width, height } = menu.getBoundingClientRect();
      menu.style.left = `${Math.max(8, Math.min(anchor.right - width, innerWidth - width - 8))}px`;
      menu.style.top = `${Math.max(8, Math.min(anchor.bottom + 6, innerHeight - height - 8))}px`;
      menu.style.visibility = "visible";
    }
    place(); panel.current?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
    const outside = (event: PointerEvent) => { if (!panel.current?.contains(event.target as Node) && !trigger.current?.contains(event.target as Node)) close(); };
    document.addEventListener("pointerdown", outside);
    window.addEventListener("resize", place);
    document.addEventListener("scroll", place, true);
    return () => { document.removeEventListener("pointerdown", outside); window.removeEventListener("resize", place); document.removeEventListener("scroll", place, true); };
  }, [open]);
  return <>
    <button ref={trigger} className="relay-icon-button owner-card-menu-trigger" type="button" aria-label={t("connections.actionsFor", { name })} aria-haspopup="menu" aria-expanded={open} aria-controls={open ? id : undefined} onClick={() => setOpen(current => !current)} onKeyDown={event => { if (event.key === "ArrowDown") { event.preventDefault(); setOpen(true); } }}><MoreHorizontal aria-hidden="true" /></button>
    {open && createPortal(<div ref={panel} id={id} className="owner-card-menu" role="menu" aria-label={t("connections.actionsFor", { name })} onPointerDown={event => { if ((event.target as HTMLElement).closest("button:disabled")) event.preventDefault(); }} onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget as Node) && event.relatedTarget !== trigger.current) close(); }} onKeyDown={event => {
      if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(true); }
      const buttons = [...event.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
      const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
      if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
        event.preventDefault(); buttons[event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1 : (index + (event.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length]?.focus();
      }
    }}>{actions.map(action => <button type="button" role="menuitem" tabIndex={-1} key={action.label} disabled={action.disabled} data-danger={action.danger || undefined} onClick={() => { close(true); action.run(); }}>{action.label}</button>)}</div>, trigger.current?.closest(".hosted-app") ?? document.body)}
  </>;
}

export function OwnerResourceCard({ connection, selected, onSelect, change, nowMs, active, onDetails, onEditSource, onSourceModels, onEditMember, currentRequests, position, totalMembers, allowReorder = true, valuesVisible = true }: {
  connection: OwnerConnection; models: CustomerModel[]; selected: boolean; onSelect: () => void;
  change: (action: ConnectionAction) => void; nowMs: number; active: boolean; onDetails?: () => void;
  position?: number; totalMembers?: number; allowReorder?: boolean; valuesVisible?: boolean;
  onEditSource?: () => void; onSourceModels?: (() => void) | undefined;
  onEditMember?: () => void;
  currentRequests?: number;
}) {
  const { t } = useLocale();
  const activityId = useId();
  const api = connection.kind === "api";
  const endpoint = sourceEndpoint(connection.source?.baseUrl);
  const status = t(connection.trashed ? "connections.inTrash" : connection.paused ? "connections.suspended" : connection.inPool ? "connections.inPool" : "connections.outsidePool");
  const error = !connection.trashed && ["failed", "requires_reauth"].includes(connection.account?.quotaStatus ?? "");
  const tone = connection.trashed || connection.paused ? "disabled" : error ? "error" : connection.inPool ? "pool" : "idle";
  const membershipLabel = t(connection.trashed ? "connections.restore" : connection.inPool ? "connections.removeFromPool" : "connections.addToPool");
  const actions = connection.trashed ? [{ label: t("connections.restoreOutsidePool"), run: () => change({ type: "restore", id: connection.id }) }] : [
    { label: t(connection.paused ? "connections.resume" : "connections.pause"), run: () => change({ type: "pause", id: connection.id }) },
    ...(position == null || !allowReorder ? [] : [
      { label: t("connections.moveUp", { name: connection.name }), disabled: position === 0, run: () => change({ type: "move", id: connection.id, direction: -1 }) },
      { label: t("connections.moveDown", { name: connection.name }), disabled: position === (totalMembers ?? 0) - 1, run: () => change({ type: "move", id: connection.id, direction: 1 }) },
    ]),
    ...(onDetails ? [{ label: t("common.details"), run: onDetails }] : []),
    ...(position == null ? [{ label: t("connections.moveToTrash"), danger: true, run: () => change({ type: "trash", id: connection.id }) }] : []),
  ];
  return <article className="owner-resource-card" aria-label={connection.name} aria-describedby={currentRequests ? activityId : undefined} data-kind={connection.kind} data-selected={selected || undefined} data-status={tone} data-current={currentRequests !== undefined ? currentRequests > 0 : undefined} data-active-requests={currentRequests}>
    {!!currentRequests && <span className="sr-only" id={activityId}>{t("pool.activity.requestsUnknown", { count: currentRequests })}</span>}
    <header className="owner-resource-card-heading">
      <span className="owner-resource-icon" data-relay-tooltip={status} role="img" aria-label={status} tabIndex={0}>{api ? <Cloud aria-hidden="true" /> : <UserRound aria-hidden="true" />}</span>
      <div className="owner-resource-identity"><h2 data-relay-tooltip={connection.name} tabIndex={0}>{connection.name}</h2><div className="owner-resource-meta">
        {connection.kind === "account" ? <><span className="owner-account-plan" data-relay-tooltip={t(connection.account?.plan ? "connections.subscription" : "connections.subscriptionUnknown")}>{connection.account?.plan ?? "—"}</span><span className="owner-account-route">{connection.account?.route ?? t("common.account")}</span></> : <span className="owner-source-endpoint" data-relay-tooltip={endpoint ?? t("source.endpointUnknown")} tabIndex={0}>{endpoint ?? "—"}</span>}
      </div></div>
      <div className="owner-resource-header-actions">{(position == null || api) && <ResourceMenu name={connection.name} active={active} actions={actions} />}<ResourceSelection label={t("connections.select", { name: connection.name })} checked={selected} onChange={onSelect} /></div>
    </header>
    <span className="sr-only">{status}</span>
    {connection.kind === "account" ? <OwnerAccountBody account={connection.account} nowMs={nowMs} valuesVisible={valuesVisible} /> : <OwnerSourceBody source={connection.source} modelCount={connection.models.length} valuesVisible={valuesVisible} onModels={onSourceModels} />}
    <footer className="owner-resource-footer">
      <button className="relay-icon-button owner-membership-action" type="button" aria-label={membershipLabel} data-relay-tooltip={membershipLabel} aria-pressed={!connection.trashed && connection.inPool} onClick={() => change({ type: connection.trashed ? "restore" : "pool", id: connection.id })}>{connection.trashed ? <RotateCcw aria-hidden="true" /> : connection.inPool ? <ListMinus aria-hidden="true" /> : <ListPlus aria-hidden="true" />}</button>
      <span className="owner-disabled-action" tabIndex={0} data-relay-tooltip={t(connection.kind === "account" ? "connections.quotaRefreshUnavailable" : "connections.sourceRefreshUnavailable")}><button className="relay-icon-button" type="button" aria-label={t(connection.kind === "account" ? "connections.refreshQuotas" : "connections.refreshSource")} disabled><RefreshCw aria-hidden="true" /></button></span>
      {api && <>
        <button className="relay-icon-button" type="button" aria-label={t("source.edit", { name: connection.name })} data-relay-tooltip={t("common.edit")} disabled={connection.trashed || !onEditSource} onClick={onEditSource}><Pencil aria-hidden="true" /></button>
        <button className="relay-icon-button" type="button" aria-label={t(connection.paused ? "connections.resume" : "connections.pause")} data-relay-tooltip={t(connection.paused ? "connections.resume" : "connections.pause")} disabled={connection.trashed} onClick={() => change({ type: "pause", id: connection.id })}>{connection.paused ? <Play aria-hidden="true" /> : <Pause aria-hidden="true" />}</button>
      </>}
      {position != null && !api && <button className="relay-icon-button" type="button" aria-label={t("pool.editMemberNamed", { name: connection.name })} data-relay-tooltip={t("common.edit")} aria-haspopup="dialog" disabled={!onEditMember} onClick={onEditMember}><Pencil aria-hidden="true" /></button>}
    </footer>
  </article>;
}
