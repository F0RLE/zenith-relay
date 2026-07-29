import { type KeyboardEvent, type PointerEvent, useEffect, useRef, useState } from "react";

export const REQUEST_COLUMN_IDS = ["time", "status", "model", "tier", "connection", "timing", "speed", "tokens", "equivalent", "request"] as const;
export type RequestColumnId = typeof REQUEST_COLUMN_IDS[number];
export type RequestTableLayout = { order: RequestColumnId[]; widths: Partial<Record<RequestColumnId, number>> };
export const MODEL_COLUMN_IDS = ["name", "requests", "input", "output", "cache", "equivalent"] as const;
export const CONNECTION_COLUMN_IDS = ["name", "requests", "success", "breakdown", "total", "equivalent", "speed", "timing"] as const;
export type AggregateColumnId = typeof MODEL_COLUMN_IDS[number] | typeof CONNECTION_COLUMN_IDS[number];
export const ERROR_COLUMN_IDS = ["time", "model", "connection", "error", "request"] as const;
export type ErrorColumnId = typeof ERROR_COLUMN_IDS[number];
type ColumnDrag<ColumnId extends string> = { column: ColumnId; pointerId: number; target: ColumnId; after: boolean };
export const REQUEST_TABLE_LAYOUT_KEY = "relay.usageRequestTableLayout";
export const REQUEST_COLUMN_MAX_WIDTH = 480;
export const REQUEST_COLUMN_MIN_WIDTH: Record<RequestColumnId, number> = { time: 130, status: 58, model: 82, tier: 110, connection: 100, timing: 96, speed: 82, tokens: 72, equivalent: 92, request: 120 };

export function loadRequestTableLayout(): RequestTableLayout {
  const fallback = { order: [...REQUEST_COLUMN_IDS], widths: {} } satisfies RequestTableLayout;
  try {
    const parsed = JSON.parse(localStorage.getItem(REQUEST_TABLE_LAYOUT_KEY) ?? "null") as { order?: unknown; widths?: Record<string, unknown> } | null;
    const order = parsed?.order;
    if (!Array.isArray(order) || order.length !== REQUEST_COLUMN_IDS.length || new Set(order).size !== REQUEST_COLUMN_IDS.length || !order.every((id) => REQUEST_COLUMN_IDS.includes(id as RequestColumnId))) return fallback;
    const widths: Partial<Record<RequestColumnId, number>> = {};
    for (const id of REQUEST_COLUMN_IDS) {
      const width = Number(parsed?.widths?.[id]);
      if (Number.isFinite(width)) widths[id] = Math.min(REQUEST_COLUMN_MAX_WIDTH, Math.max(REQUEST_COLUMN_MIN_WIDTH[id], Math.round(width)));
    }
    return { order: order as RequestColumnId[], widths: Object.keys(widths).length === REQUEST_COLUMN_IDS.length ? widths : {} };
  } catch {
    return fallback;
  }
}

export function reorderColumns<ColumnId extends string>(order: ColumnId[], column: ColumnId, target: ColumnId, after = false) {
  if (column === target) return order;
  const next = order.filter((id) => id !== column);
  next.splice(next.indexOf(target) + Number(after), 0, column);
  return next;
}

export function shiftColumn<ColumnId extends string>(order: ColumnId[], column: ColumnId, offset: number) {
  const from = order.indexOf(column);
  const to = Math.min(order.length - 1, Math.max(0, from + offset));
  if (from === to) return order;
  const next = [...order];
  next.splice(to, 0, next.splice(from, 1)[0]);
  return next;
}

export function useStoredColumnOrder<ColumnId extends string>(storageKey: string, defaults: readonly ColumnId[]) {
  const [order, setOrder] = useState<ColumnId[]>(() => {
    try {
      const stored = JSON.parse(localStorage.getItem(storageKey) ?? "null") as unknown;
      if (Array.isArray(stored) && stored.length === defaults.length && new Set(stored).size === defaults.length && stored.every((id) => defaults.includes(id as ColumnId))) return stored as ColumnId[];
    } catch { }
    return [...defaults];
  });
  useEffect(() => {
    try { localStorage.setItem(storageKey, JSON.stringify(order)); } catch { }
  }, [order, storageKey]);
  return [order, setOrder] as const;
}

export function useColumnDrag<ColumnId extends string>(moveColumn: (column: ColumnId, target: ColumnId, after: boolean) => void, moveColumnBy: (column: ColumnId, offset: number) => void) {
  const [drag, setDrag] = useState<ColumnDrag<ColumnId> | null>(null);
  const dragRef = useRef<ColumnDrag<ColumnId> | null>(null);
  const cancel = () => { dragRef.current = null; setDrag(null); };
  const bind = (column: ColumnId) => ({
    onPointerDown: (event: PointerEvent<HTMLButtonElement>) => {
      if (event.button !== 0) return;
      event.preventDefault();
      event.currentTarget.setPointerCapture(event.pointerId);
      const next = { column, pointerId: event.pointerId, target: column, after: false };
      dragRef.current = next;
      setDrag(next);
    },
    onPointerMove: (event: PointerEvent<HTMLButtonElement>) => {
      const current = dragRef.current;
      const table = event.currentTarget.closest("table");
      if (!current || current.pointerId !== event.pointerId || !(table instanceof HTMLTableElement)) return;
      const headers = Array.from(table.querySelectorAll<HTMLTableCellElement>("thead th[data-column]"));
      const target = headers.find((header) => event.clientX <= header.getBoundingClientRect().right) ?? headers[headers.length - 1];
      if (!target) return;
      const bounds = target.getBoundingClientRect();
      const targetId = target.dataset.column as ColumnId;
      const after = event.clientX > bounds.left + bounds.width / 2;
      if (current.target === targetId && current.after === after) return;
      const next = { ...current, target: targetId, after };
      dragRef.current = next;
      setDrag(next);
    },
    onPointerUp: (event: PointerEvent<HTMLButtonElement>) => {
      const current = dragRef.current;
      if (!current || current.pointerId !== event.pointerId) return;
      if (current.column !== current.target) moveColumn(current.column, current.target, current.after);
      cancel();
      if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    },
    onPointerCancel: cancel,
    onLostPointerCapture: () => { if (dragRef.current?.column === column) cancel(); },
    onKeyDown: (event: KeyboardEvent<HTMLButtonElement>) => {
      if (!event.altKey || (event.key !== "ArrowLeft" && event.key !== "ArrowRight")) return;
      event.preventDefault();
      moveColumnBy(column, event.key === "ArrowLeft" ? -1 : 1);
    },
  });
  return { bind, drag };
}
