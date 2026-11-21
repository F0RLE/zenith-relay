import { type KeyboardEvent, type PointerEvent, useEffect, useRef, useState } from "react";

export const REQUEST_COLUMN_IDS = ["time", "status", "model", "reasoning", "connection", "timing", "speed", "tokens", "equivalent", "request"] as const;
export type RequestColumnId = typeof REQUEST_COLUMN_IDS[number];
export type RequestTableLayout = { order: RequestColumnId[] };
export const MODEL_COLUMN_IDS = ["name", "requests", "input", "output", "cache", "equivalent"] as const;
export const CONNECTION_COLUMN_IDS = ["name", "requests", "success", "breakdown", "total", "equivalent", "speed", "timing"] as const;
export type AggregateColumnId = typeof MODEL_COLUMN_IDS[number] | typeof CONNECTION_COLUMN_IDS[number];
export const ERROR_COLUMN_IDS = ["time", "model", "connection", "origin", "error", "request"] as const;
export type ErrorColumnId = typeof ERROR_COLUMN_IDS[number];
type ColumnDrag<ColumnId extends string> = { column: ColumnId; pointerId: number; target: ColumnId; after: boolean };
export const REQUEST_TABLE_LAYOUT_KEY = "relay.usageRequestTableLayout";
export function loadRequestTableLayout(): RequestTableLayout {
  const fallback = { order: [...REQUEST_COLUMN_IDS] } satisfies RequestTableLayout;
  try {
    const parsed = JSON.parse(localStorage.getItem(REQUEST_TABLE_LAYOUT_KEY) ?? "null") as { order?: unknown } | null;
    const storedOrder = parsed?.order;
    if (!Array.isArray(storedOrder) || !storedOrder.every((columnId) => typeof columnId === "string") || new Set(storedOrder).size !== storedOrder.length) return fallback;
    const order = storedOrder.filter((columnId): columnId is RequestColumnId => REQUEST_COLUMN_IDS.includes(columnId as RequestColumnId));
    for (const columnId of REQUEST_COLUMN_IDS) {
      if (!order.includes(columnId)) {
        // Place new fields beside their related columns, preserving saved order.
        const index = columnId === "speed" ? order.indexOf("timing") + 1 : columnId === "reasoning" ? order.indexOf("model") + 1 : order.length;
        order.splice(index, 0, columnId);
      }
    }
    return { order };
  } catch {
    return fallback;
  }
}

export function reorderColumns<ColumnId extends string>(order: ColumnId[], column: ColumnId, target: ColumnId, after = false) {
  if (column === target) return order;
  const reorderedColumns = order.filter((columnId) => columnId !== column);
  reorderedColumns.splice(reorderedColumns.indexOf(target) + Number(after), 0, column);
  return reorderedColumns;
}

export function shiftColumn<ColumnId extends string>(order: ColumnId[], column: ColumnId, offset: number) {
  const from = order.indexOf(column);
  const to = Math.min(order.length - 1, Math.max(0, from + offset));
  if (from === to) return order;
  const shiftedColumns = [...order];
  const [moved] = shiftedColumns.splice(from, 1);
  if (moved === undefined) return order;
  shiftedColumns.splice(to, 0, moved);
  return shiftedColumns;
}

export function useStoredColumnOrder<ColumnId extends string>(storageKey: string, defaults: readonly ColumnId[]) {
  const [order, setOrder] = useState<ColumnId[]>(() => {
    try {
      const stored = JSON.parse(localStorage.getItem(storageKey) ?? "null") as unknown;
      if (Array.isArray(stored) && stored.length === defaults.length && new Set(stored).size === defaults.length && stored.every((columnId) => defaults.includes(columnId as ColumnId))) return stored as ColumnId[];
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
      const initialDrag = { column, pointerId: event.pointerId, target: column, after: false };
      dragRef.current = initialDrag;
      setDrag(initialDrag);
    },
    onPointerMove: (event: PointerEvent<HTMLButtonElement>) => {
      const activeDrag = dragRef.current;
      const table = event.currentTarget.closest("table");
      if (!activeDrag || activeDrag.pointerId !== event.pointerId || !(table instanceof HTMLTableElement)) return;
      const headers = Array.from(table.querySelectorAll<HTMLTableCellElement>("thead th[data-column]"));
      const target = headers.find((header) => event.clientX <= header.getBoundingClientRect().right) ?? headers[headers.length - 1];
      if (!target) return;
      const bounds = target.getBoundingClientRect();
      const targetId = target.dataset["column"] as ColumnId;
      const after = event.clientX > bounds.left + bounds.width / 2;
      if (activeDrag.target === targetId && activeDrag.after === after) return;
      const updatedDrag = { ...activeDrag, target: targetId, after };
      dragRef.current = updatedDrag;
      setDrag(updatedDrag);
    },
    onPointerUp: (event: PointerEvent<HTMLButtonElement>) => {
      const activeDrag = dragRef.current;
      if (!activeDrag || activeDrag.pointerId !== event.pointerId) return;
      if (activeDrag.column !== activeDrag.target) moveColumn(activeDrag.column, activeDrag.target, activeDrag.after);
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
