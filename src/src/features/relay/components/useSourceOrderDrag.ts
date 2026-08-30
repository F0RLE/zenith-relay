import { useCallback, useRef, useState, type PointerEvent } from "react";
import type { ApiSourceRole } from "../routingOrder";
import { usePointerDragListeners } from "../hooks/usePointerDragListeners";

type SourceOrderDragInput = {
  memberId: string;
  sourceRole: ApiSourceRole;
  onRoleDrop: (role: ApiSourceRole) => void;
  onSourceDrop: (sourceId: string, targetId: string, after: boolean) => void;
};

type SourceDragRef = {
  pointerId: number;
  sourceId: string;
  clientX: number;
  clientY: number;
};

export function useSourceOrderDrag({
  memberId,
  sourceRole,
  onRoleDrop,
  onSourceDrop,
}: SourceOrderDragInput) {
  const [draggedSource, setDraggedSource] = useState<string | null>(null);
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const [dropAfter, setDropAfter] = useState(false);
  const [dropRole, setDropRole] = useState<ApiSourceRole | null>(null);
  const dragRef = useRef<SourceDragRef | null>(null);

  const clearSourceDrag = useCallback(() => {
    dragRef.current = null;
    setDraggedSource(null);
    setDropTarget(null);
    setDropAfter(false);
    setDropRole(null);
  }, []);

  const updateSourceDragAt = useCallback((clientX: number, clientY: number) => {
    const sourceId = dragRef.current?.sourceId;
    if (!sourceId) return;
    const target = document.elementFromPoint(clientX, clientY);
    const role = sourceId === memberId ? sourceRoleAt(target) : null;
    if (role) {
      setDropRole(role);
      setDropTarget(null);
      setDropAfter(false);
      return;
    }
    const row = target?.closest<HTMLElement>("[data-source-id]");
    const targetId = row?.dataset.sourceId ?? null;
    setDropRole(null);
    setDropTarget(targetId && targetId !== sourceId ? targetId : null);
    if (targetId && targetId !== sourceId && row) {
      const bounds = row.getBoundingClientRect();
      setDropAfter(clientY >= bounds.top + bounds.height / 2);
    } else {
      setDropAfter(false);
    }
  }, [memberId, sourceRole]);

  const finishSourceDragAt = useCallback((clientX: number, clientY: number) => {
    const sourceId = dragRef.current?.sourceId;
    if (!sourceId) return;
    const target = document.elementFromPoint(clientX, clientY);
    const role = sourceId === memberId ? sourceRoleAt(target) : null;
    if (role) {
      onRoleDrop(role);
      clearSourceDrag();
      return;
    }
    const row = target?.closest<HTMLElement>("[data-source-id]");
    const targetId = row?.dataset.sourceId;
    if (targetId && targetId !== sourceId && row) {
      const bounds = row.getBoundingClientRect();
      onSourceDrop(sourceId, targetId, clientY >= bounds.top + bounds.height / 2);
    }
    clearSourceDrag();
  }, [clearSourceDrag, memberId, onRoleDrop, onSourceDrop]);

  usePointerDragListeners({
    dragRef,
    activeKey: draggedSource ? `source:${draggedSource}` : null,
    onMove: (_drag, clientX, clientY) => updateSourceDragAt(clientX, clientY),
    onDrop: (_drag, clientX, clientY) => finishSourceDragAt(clientX, clientY),
    onCancel: clearSourceDrag,
  });

  const startSourceDrag = useCallback((event: PointerEvent<HTMLButtonElement>, sourceId: string) => {
    if (event.button !== 0) return;
    event.preventDefault();
    dragRef.current = {
      pointerId: event.pointerId,
      sourceId,
      clientX: event.clientX,
      clientY: event.clientY,
    };
    setDraggedSource(sourceId);
    setDropTarget(null);
    setDropAfter(false);
    setDropRole(null);
  }, []);

  return {
    draggedSource,
    dropTarget,
    dropAfter,
    dropRole,
    clearSourceDrag,
    startSourceDrag,
  };
}

function sourceRoleAt(target: Element | null): ApiSourceRole | null {
  const role = target?.closest<HTMLElement>("[data-source-role]")?.dataset.sourceRole;
  return role === "primary" || role === "stabilizer" || role === "reserve" ? role : null;
}
