import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent, PointerEvent } from "react";
import type { CatalogMove, catalogGroups } from "../models/catalogOrder";

type Item = { kind: CatalogMove["kind"]; id: string };
type Drag = Item & { pointerId: number; clientX: number; clientY: number };

/** Handle-only pointer/touch dragging; price inputs and switches never start a drag. */
export function useCatalogReorder(groups: ReturnType<typeof catalogGroups>, onMove: (move: CatalogMove) => void, active: boolean) {
  const root = useRef<HTMLElement>(null);
  const pointer = useRef<Drag | null>(null);
  const [drag, setDrag] = useState<Item | null>(null);
  const [target, setTarget] = useState<string | null>(null);
  function clear() { pointer.current = null; setDrag(null); setTarget(null); }
  function peers(item: Item) {
    return item.kind === "group" ? groups : groups.find(group => group.models.some(model => model.id === item.id))?.models ?? [];
  }
  function targetAt(item: Item, x: number, y: number) {
    const element = document.elementFromPoint(x, y);
    if (!element || !root.current?.contains(element)) return null;
    const row = element.closest<HTMLElement>(item.kind === "group" ? "[data-group-id]" : ".owner-model-row[data-model-id]");
    const id = row?.dataset[item.kind === "group" ? "groupId" : "modelId"];
    return id && id !== item.id && peers(item).some(peer => peer.id === id) ? id : null;
  }
  // Keep the React boundary in web; Desktop hooks resolve a different React installation.
  const handlers = useRef({ targetAt, onMove });
  handlers.current = { targetAt, onMove };
  useEffect(() => { if (!active) clear(); }, [active]);
  useEffect(() => {
    if (!drag) return;
    const movePointer = (event: globalThis.PointerEvent) => {
      const item = pointer.current;
      if (!item || item.pointerId !== event.pointerId) return;
      item.clientX = event.clientX;
      item.clientY = event.clientY;
      setTarget(handlers.current.targetAt(item, event.clientX, event.clientY));
    };
    const drop = (event: globalThis.PointerEvent) => {
      const item = pointer.current;
      if (!item || item.pointerId !== event.pointerId) return;
      const destination = handlers.current.targetAt(item, event.clientX, event.clientY);
      clear();
      if (destination) handlers.current.onMove({ kind: item.kind, source: item.id, target: destination });
    };
    const cancel = (event: globalThis.PointerEvent) => { if (pointer.current?.pointerId === event.pointerId) clear(); };
    let frame = 0;
    const track = () => {
      const item = pointer.current;
      if (!item) return;
      const scrollPane = root.current?.closest<HTMLElement>(".relay-content");
      const box = scrollPane?.getBoundingClientRect();
      if (scrollPane && box && item.clientX >= box.left && item.clientX <= box.right) {
        const edge = 48;
        const direction = item.clientY < Math.max(0, box.top) + edge ? -1 : item.clientY > Math.min(innerHeight, box.bottom) - edge ? 1 : 0;
        if (direction) scrollPane.scrollTop += direction * 8;
      }
      setTarget(handlers.current.targetAt(item, item.clientX, item.clientY));
      frame = requestAnimationFrame(track);
    };
    frame = requestAnimationFrame(track);
    const escape = (event: globalThis.KeyboardEvent) => { if (event.key === "Escape") { event.preventDefault(); clear(); } };
    window.addEventListener("keydown", escape);
    window.addEventListener("blur", clear);
    window.addEventListener("pointermove", movePointer);
    window.addEventListener("pointerup", drop);
    window.addEventListener("pointercancel", cancel);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("keydown", escape); window.removeEventListener("blur", clear);
      window.removeEventListener("pointermove", movePointer); window.removeEventListener("pointerup", drop); window.removeEventListener("pointercancel", cancel);
    };
  }, [drag]);
  function start(event: PointerEvent<HTMLButtonElement>, item: Item) {
    if (!active || event.button !== 0 || !event.isPrimary || peers(item).length < 2) return;
    event.preventDefault();
    event.currentTarget.focus({ preventScroll: true });
    pointer.current = { ...item, pointerId: event.pointerId, clientX: event.clientX, clientY: event.clientY };
    setDrag(item);
    setTarget(null);
  }
  function key(event: KeyboardEvent<HTMLButtonElement>, item: Item) {
    if (!active || drag || !["ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    const list = peers(item);
    const index = list.findIndex(peer => peer.id === item.id);
    const next = event.key === "Home" ? 0 : event.key === "End" ? list.length - 1 : index + (event.key === "ArrowUp" ? -1 : 1);
    const destination = list[next];
    if (index >= 0 && destination && destination.id !== item.id) {
      onMove({ kind: item.kind, source: item.id, target: destination.id });
      const button = event.currentTarget;
      requestAnimationFrame(() => button.scrollIntoView({ block: "nearest" }));
    }
  }
  function state(kind: Item["kind"], id: string) {
    const isTarget = drag?.kind === kind && target === id;
    return { "data-dragging": drag?.kind === kind && drag.id === id || undefined, "data-drop-target": isTarget || undefined };
  }
  return { root, start, key, state };
}
