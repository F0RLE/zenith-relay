import { useEffect, useId, useRef, useState } from "react";
import { groupModels } from "../../../../../desktop/frontend/src/features/relay/modelGroups";
import { readModelDisplayOrder, rulesByModel, saveModelDisplayOrder, type ModelDisplayOrder, type Rule } from "../../api/catalog";
import { ControlError, observeControl } from "../../api/control";
import { useLocale } from "../../i18n/LocaleProvider";
import { moveCatalogBlock, type CatalogMove } from "../models/catalogOrder";
import { ModelGroupHeading, ModelOrderGrip } from "./ModelOrderControls";
import { ModelPriceEditor } from "./ModelPriceEditor";
import { useCatalogReorder } from "./useCatalogReorder";
import type { ApplyRule } from "./useOwnerRules";
import "./owner-model-rules.css";
import "./live-model-rules.css";

type MoveIntent = { move: CatalogMove; after: boolean };

export function PoolModelRules({ rows, csrf, apply, blocked, editable }: {
  rows: Rule[]; csrf: string; apply: ApplyRule; blocked: boolean; editable: boolean;
}) {
  const { t } = useLocale();
  const [layout, setLayout] = useState<ModelDisplayOrder>();
  const [failed, setFailed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [reload, setReload] = useState(0);
  const [intent, setIntent] = useState<MoveIntent>();
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());
  const dragHelp = useId(), groupList = useId();
  const [announcement, setAnnouncement] = useState("");
  const writing = useRef(false), epoch = useRef(0);
  const refresh = () => { epoch.current++; setReload(current => current + 1); };
  useEffect(() => observeControl(async () => {
    const version = epoch.current;
    if (writing.current) return;
    try { return { version, value: await readModelDisplayOrder() }; }
    catch (error) { return { version, error }; }
  }, result => {
    if (!result || result.version !== epoch.current || writing.current) return;
    if ("value" in result) {
      setLayout(result.value);
      setFailed(false);
    } else if (result.error instanceof ControlError && result.error.status === 401) location.replace("/admin/login");
    else setFailed(true);
  }, () => setFailed(true)), [rows, reload]);

  const visible = rulesByModel(rows.filter(row => row.admitted || row.enabled));
  const inventory = groupModels(layout?.items.map(item => ({ id: item.model, publisher: item.publisher })) ?? [],
    { metadata: item => ({ catalogProvider: item.publisher }) });
  const visibleGroups = inventory.map(group => ({ ...group, models: group.models.filter(model => visible.has(model.id)) }))
    .filter(group => group.models.length);
  const known = new Set(layout?.items.map(item => item.model));
  // Mount price editors only once their authoritative group is known.
  const waiting = [...visible.keys()].some(id => !known.has(id));
  const available = editable && !!layout && !waiting && !failed;
  const displayName = (group: (typeof visibleGroups)[number]) => group.provider === "other" ? t("modelCatalog.otherCompany") : group.label;
  const position = (move: CatalogMove) => {
    const peers = move.kind === "group" ? visibleGroups
      : visibleGroups.find(group => group.models.some(model => model.id === move.source))?.models ?? [];
    const from = peers.findIndex(item => item.id === move.source);
    const to = peers.findIndex(item => item.id === move.target);
    return from >= 0 && to >= 0 ? { after: from > to } : null;
  };
  const retry = intent && position(intent.move);
  const alreadyApplied = retry && retry.after === intent?.after;

  async function move(item: CatalogMove, retrying = false) {
    if (!layout || !available || blocked || writing.current) return;
    const next = moveCatalogBlock(inventory, item);
    const prior = position(item);
    if (!next || !prior) return;
    const pending = retrying && intent ? intent : { move: item, after: !prior.after };
    const focused = document.activeElement;
    writing.current = true; epoch.current++; setBusy(true); setIntent(undefined);
    try {
      setLayout(await saveModelDisplayOrder(csrf, layout.revision, next));
      setAnnouncement(t("modelRules.orderSaved"));
      if (focused instanceof HTMLButtonElement) requestAnimationFrame(() => {
        if (!focused.isConnected || document.activeElement !== document.body && document.activeElement !== focused) return;
        focused.focus({ preventScroll: true });
        focused.scrollIntoView({ block: "nearest" });
      });
    } catch (error) {
      if (error instanceof ControlError && error.status === 401) location.replace("/admin/login");
      else { setIntent(pending); setFailed(true); refresh(); }
    } finally { writing.current = false; setBusy(false); }
  }

  const reorder = useCatalogReorder(visibleGroups, move, available && !blocked && !busy);
  const handle = (kind: CatalogMove["kind"], id: string, name: string, count: number) =>
    <ModelOrderGrip kind={kind} id={id} name={name} available={editable && count > 1}
      busy={!available || blocked || busy} help={dragHelp} reorder={reorder} />;

  return <section ref={reorder.root} className="entry-catalog live-model-rules" aria-label={t("modelRules.title")}>
    <span id={dragHelp} className="sr-only">{t("modelRules.dragHelp")}</span>
    <span className="sr-only" aria-live="polite" aria-atomic="true">{announcement}</span>
    {(failed || intent && !alreadyApplied) && <div className="catalog-actions">
      <small role="alert">{t(failed ? "modelRules.orderUnavailable" : "modelRules.orderChanged")}</small>
      {failed ? <button className="key-button key-secondary" type="button" onClick={refresh}>{t("common.retry")}</button>
        : intent && <>
          {retry && <button className="key-button key-secondary" type="button" disabled={!available || blocked || busy}
            onClick={() => void move(intent.move, true)}>{t("common.retry")}</button>}
          <button className="key-button key-secondary" type="button" onClick={() => setIntent(undefined)}>{t("common.close")}</button>
        </>}
    </div>}
    {visibleGroups.length ? <div className="owner-model-rules live-rule-frame">
      {visibleGroups.map(group => {
        const listId = `${groupList}-${group.id}`, expanded = !collapsed.has(group.id);
        const name = displayName(group);
        return <div className="live-rule-group" key={group.id} data-group-id={group.id} {...reorder.state("group", group.id)}>
          <div className="live-rule-heading">
            <ModelGroupHeading name={name} count={group.models.length} controls={listId} expanded={expanded}
              grip={handle("group", group.id, name, visibleGroups.length)}
              toggle={() => setCollapsed(current => {
                const next = new Set(current);
                if (next.has(group.id)) next.delete(group.id); else next.add(group.id);
                return next;
              })} />
          </div>
          <ul id={listId} className="catalog-list" hidden={!expanded}>
            {group.models.map(model => <ModelPriceEditor key={model.id} sources={visible.get(model.id)!} busy={blocked} apply={apply}
              grip={handle("model", model.id, model.id, group.models.length)}
              row={{ "data-model-id": model.id, ...reorder.state("model", model.id) }} />)}
          </ul>
        </div>;
      })}
      <div className="catalog-unit">{t("modelRules.units")}</div>
    </div> : !waiting && <p className="owner-resource-empty" role="status">{t("modelRules.empty")}</p>}
    {waiting && !failed && <p className="owner-resource-empty" role="status">{t("unavailable.connecting")}</p>}
  </section>;
}
