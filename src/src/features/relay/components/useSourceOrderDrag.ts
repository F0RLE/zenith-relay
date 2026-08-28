import { useCallback, useEffect, useRef, useState, type PointerEvent } from "react";
import type { ApiSourceRole } from "../routingOrder";

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

  useEffect(() => {
    const drag = dragRef.current;
    if (!drag || draggedSource !== drag.sourceId) return;
    const onPointerMove = (event: globalThis.PointerEvent) => {
      if (event.pointerId !== drag.pointerId) return;
      drag.clientX = event.clientX;
      drag.clientY = event.clientY;
      updateSourceDragAt(event.clientX, event.clientY);
    };
    const onPointerUp = (event: globalThis.PointerEvent) => {
      if (event.pointerId !== drag.pointerId) return;
      finishSourceDragAt(event.clientX, event.clientY);
    };
    const onPointerCancel = (event: globalThis.PointerEvent) => {
      if (event.pointerId === drag.pointerId) clearSourceDrag();
    };
    const onWheel = () => {
      // Scrolling is allowed during a drag; refresh the indicator after the
      // row positions have been laid out again.
      requestAnimationFrame(() => {
        if (dragRef.current !== drag) return;
        updateSourceDragAt(drag.clientX, drag.clientY);
      });
    };
    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", onPointerUp);
    window.addEventListener("pointercancel", onPointerCancel);
    window.addEventListener("wheel", onWheel, { passive: true });
    return () => {
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", onPointerUp);
      window.removeEventListener("pointercancel", onPointerCancel);
      window.removeEventListener("wheel", onWheel);
    };
  }, [clearSourceDrag, draggedSource, finishSourceDragAt, updateSourceDragAt]);

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
