import { useEffect, useLayoutEffect, useRef } from "react";
import type { RefObject } from "react";
import { ArrowUpRight, CheckCheck, Clock3, X } from "lucide-react";

export function NotificationPanel({ anchor, read, modelName, onRead, onOpenRequest, onClose }: {
  anchor: RefObject<HTMLButtonElement | null>;
  read: boolean;
  modelName: string;
  onRead: () => void;
  onOpenRequest: () => void;
  onClose: () => void;
}) {
  const panel = useRef<HTMLElement>(null);
  function close(restoreFocus = false) {
    if (restoreFocus) anchor.current?.focus({ preventScroll: true });
    onClose();
  }
  useLayoutEffect(() => {
    const place = () => {
      const trigger = anchor.current?.getBoundingClientRect();
      const root = panel.current;
      if (!trigger || !root) return;
      const width = Math.min(400, innerWidth - 24);
      root.style.width = `${width}px`;
      root.style.maxHeight = `${innerHeight - 24}px`;
      const below = Math.max(0, innerHeight - trigger.bottom - 22);
      const above = Math.max(0, trigger.top - 22);
      const down = root.getBoundingClientRect().height <= below || below >= above;
      root.style.maxHeight = `${down ? below : above}px`;
      const height = root.getBoundingClientRect().height;
      root.style.left = `${Math.max(12, Math.min(trigger.right - width, innerWidth - width - 12))}px`;
      root.style.top = `${Math.max(12, down ? trigger.bottom + 10 : trigger.top - height - 10)}px`;
      root.style.visibility = "visible";
    };
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [anchor]);
  useEffect(() => {
    panel.current?.querySelector<HTMLButtonElement>("button")?.focus({ preventScroll: true });
  }, []);
  useEffect(() => {
    const outside = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!panel.current?.contains(target) && !anchor.current?.contains(target)) onClose();
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        anchor.current?.focus({ preventScroll: true });
        onClose();
      }
    };
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("keydown", escape, true);
    return () => {
      document.removeEventListener("pointerdown", outside, true);
      document.removeEventListener("keydown", escape, true);
    };
  }, [anchor, onClose]);
  return <section ref={panel} id="notifications-panel" className="notification-panel" role="dialog" aria-labelledby="notifications-title"
    onBlur={event => {
      const next = event.relatedTarget;
      if (next instanceof Node && !event.currentTarget.contains(next) && !anchor.current?.contains(next)) onClose();
    }}>
    <header className="notification-panel-header"><div><h2 id="notifications-title">Уведомления</h2><span>{read ? "Нет новых" : "1 новое"}</span></div>
      <button type="button" className="notification-close" aria-label="Закрыть уведомления" onClick={() => close(true)}><X aria-hidden="true" /></button>
    </header>
    <article className="notification-entry" data-unread={!read}>
      <span className="notification-event-icon"><Clock3 aria-hidden="true" /></span>
      <div className="notification-entry-content"><div className="notification-entry-heading"><h3>Расчёт ещё уточняется</h3>{!read && <span className="notification-unread-dot" aria-hidden="true" />}</div>
        <p>Ответ готов. Итоговые токены и сумма ещё уточняются. Повторять запрос не нужно.</p>
        <div className="notification-entry-meta"><span>{modelName}</span>
          <button type="button" className="notification-open" onClick={() => { anchor.current?.focus({ preventScroll: true }); onOpenRequest(); }}>Открыть запрос<ArrowUpRight aria-hidden="true" /></button>
        </div>
      </div>
    </article>
    <footer className="notification-panel-footer"><span>Демо</span><button type="button" className="notification-mark-read" aria-label="Отметить прочитанным" disabled={read} onClick={onRead}><CheckCheck aria-hidden="true" />{read ? "Всё прочитано" : "Прочитать всё"}</button></footer>
  </section>;
}
