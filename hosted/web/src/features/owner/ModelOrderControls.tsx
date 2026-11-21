import { useLocale } from "../../i18n/LocaleProvider";
import { ChevronDown, GripVertical } from "../../components/Icons";
import type { CatalogMove } from "../models/catalogOrder";
import type { ReactNode } from "react";
import type { useCatalogReorder } from "./useCatalogReorder";

export function ModelOrderGrip({ kind, id, name, help, available, busy = false, reorder }: {
  kind: CatalogMove["kind"]; id: string; name: string; help: string;
  available: boolean; busy?: boolean; reorder: ReturnType<typeof useCatalogReorder>;
}) {
  const { t } = useLocale();
  return <button type="button" className="owner-model-drag-handle" data-kind={kind}
    disabled={!available} aria-disabled={busy || undefined}
    aria-label={t(kind === "group" ? "modelRules.moveCompany" : "modelRules.moveModel", { name })}
    aria-describedby={help} aria-keyshortcuts="ArrowUp ArrowDown Home End" data-relay-tooltip={t("modelRules.dragHint")}
    onPointerDown={event => { if (!busy) reorder.start(event, { kind, id }); }}
    onKeyDown={event => { if (!busy) reorder.key(event, { kind, id }); }}>
    <GripVertical aria-hidden="true" />
  </button>;
}

export function ModelGroupHeading({ name, count, expanded, controls, toggle, grip }: {
  name: string; count: number; expanded: boolean; controls: string; toggle: () => void; grip: ReactNode;
}) {
  const { t, locale } = useLocale();
  return <div className="owner-model-group-heading">
    <button type="button" className="owner-model-group-toggle" aria-expanded={expanded}
      aria-label={t(expanded ? "modelRules.collapseCompany" : "modelRules.expandCompany", { name })}
      aria-controls={controls} onClick={toggle}><ChevronDown aria-hidden="true" /></button>
    {grip}
    <strong>{name}</strong><small>{count.toLocaleString(locale)}</small>
  </div>;
}
