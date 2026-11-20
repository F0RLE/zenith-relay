import { useEffect, useLayoutEffect, useRef } from "react";
import type { ReactNode, RefObject } from "react";
import { CheckCheck, X } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import "./notifications.css";

export function NotificationPopover({ anchor, unread, onRead, reading, onClose, children }: {
  anchor: RefObject<HTMLButtonElement | null>;
  unread: number; onRead: () => void; reading?: boolean; onClose: () => void; children: ReactNode;
}) {
  const { t, locale } = useLocale();
  const panel = useRef<HTMLElement>(null);
  const closeButton = useRef<HTMLButtonElement>(null);

  useLayoutEffect(() => {
    const root = panel.current;
    if (!root) return;
    const place = () => {
      const trigger = anchor.current?.getBoundingClientRect();
      if (!trigger) return;
      const width = Math.min(384, Math.max(0, innerWidth - 24));
      root.style.width = `${width}px`;
      const below = Math.max(0, innerHeight - trigger.bottom - 22);
      const above = Math.max(0, trigger.top - 22);
      const down = root.scrollHeight + 2 <= below || below >= above;
      root.style.maxHeight = `${down ? below : above}px`;
      root.style.left = `${Math.max(12, Math.min(trigger.right - width, innerWidth - width - 12))}px`;
      root.style.top = `${Math.max(12, down ? trigger.bottom + 10 : trigger.top - root.getBoundingClientRect().height - 10)}px`;
      root.style.visibility = "visible";
    };
    place();
    const observer = new ResizeObserver(place);
    observer.observe(root);
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [anchor, locale]);

  useEffect(() => { closeButton.current?.focus({ preventScroll: true }); }, []);
  useEffect(() => {
    const outside = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!panel.current?.contains(target) && !anchor.current?.contains(target)) onClose();
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      anchor.current?.focus({ preventScroll: true });
      onClose();
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
    <header className="notification-panel-header"><div><h2 id="notifications-title">{t("shell.notifications")}</h2>
      {unread > 0 && <span className="notification-count" aria-label={unread === 1 ? t("notifications.oneNew")
        : t("notifications.unreadCount", { count: unread.toLocaleString(locale) })}>{unread.toLocaleString(locale)}</span>}</div>
      <div className="notification-panel-actions">
        {unread > 0 && <button type="button" className="notification-mark-read" disabled={reading}
          aria-label={t("notifications.markAsRead")} data-relay-tooltip={t("notifications.readAll")}
          onClick={() => { closeButton.current?.focus({ preventScroll: true }); onRead(); }}><CheckCheck aria-hidden="true" /></button>}
        <button ref={closeButton} type="button" className="notification-close" aria-label={t("notifications.closeNotifications")}
          onClick={() => { anchor.current?.focus({ preventScroll: true }); onClose(); }}><X aria-hidden="true" /></button>
      </div>
    </header>
    {children}
  </section>;
}
