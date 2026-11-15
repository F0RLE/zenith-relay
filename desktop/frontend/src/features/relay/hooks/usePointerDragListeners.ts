import { useEffect, useRef } from "react";

export type PointerDragPosition = {
  pointerId: number;
  clientX: number;
  clientY: number;
};

type PointerDragRef<T extends PointerDragPosition> = {
  current: T | null;
};

type PointerDragListenersInput<T extends PointerDragPosition> = {
  dragRef: PointerDragRef<T>;
  activeKey: string | null;
  onMove: (drag: T, clientX: number, clientY: number) => void;
  onDrop: (drag: T, clientX: number, clientY: number) => void;
  onCancel: (drag: T) => void;
};

/** Maintains global pointer listeners while a list/table drag is active. */
export function usePointerDragListeners<T extends PointerDragPosition>({
  dragRef,
  activeKey,
  onMove,
  onDrop,
  onCancel,
}: PointerDragListenersInput<T>) {
  const handlersRef = useRef({ onMove, onDrop, onCancel });
  handlersRef.current = { onMove, onDrop, onCancel };

  useEffect(() => {
    const drag = dragRef.current;
    if (!drag || !activeKey) return;

    const onPointerMove = (event: globalThis.PointerEvent) => {
      if (event.pointerId !== drag.pointerId) return;
      drag.clientX = event.clientX;
      drag.clientY = event.clientY;
      handlersRef.current.onMove(drag, event.clientX, event.clientY);
    };
    const onPointerUp = (event: globalThis.PointerEvent) => {
      if (event.pointerId !== drag.pointerId) return;
      handlersRef.current.onDrop(drag, event.clientX, event.clientY);
    };
    const onPointerCancel = (event: globalThis.PointerEvent) => {
      if (event.pointerId === drag.pointerId) handlersRef.current.onCancel(drag);
    };
    const onWheel = () => {
      // Scrolling stays available during a drag; update the visual target
      // after the browser has laid out the rows at their new positions.
      requestAnimationFrame(() => {
        if (dragRef.current === drag) {
          handlersRef.current.onMove(drag, drag.clientX, drag.clientY);
        }
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
  }, [activeKey, dragRef]);
}
