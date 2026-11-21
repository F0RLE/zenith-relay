import type { RefObject } from "react";
import { ArrowRight, Clock3 } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import { NotificationPopover } from "./NotificationPopover";

export function NotificationPanel({ anchor, read, modelName, onRead, onOpenRequest, onClose }: {
  anchor: RefObject<HTMLButtonElement | null>;
  read: boolean;
  modelName: string;
  onRead: () => void;
  onOpenRequest: () => void;
  onClose: () => void;
}) {
  const { t } = useLocale();
  return <NotificationPopover anchor={anchor} unread={read ? 0 : 1} onRead={onRead} onClose={onClose}>
    <article className="notification-entry" data-unread={!read}>
      <span className="notification-event-icon"><Clock3 aria-hidden="true" /></span>
      <div className="notification-entry-content"><div className="notification-entry-heading"><h3>{t("notifications.accountingIsStillBeingFinalized")}</h3>{!read && <span className="notification-unread-dot" aria-hidden="true" />}</div>
        <p>{t("notifications.pendingNotice")}</p>
        <div className="notification-entry-meta"><span>{modelName}</span>
          <button type="button" className="notification-open" onClick={() => { anchor.current?.focus({ preventScroll: true }); onOpenRequest(); }}>{t("notifications.openRequest")}<ArrowRight aria-hidden="true" /></button>
        </div>
      </div>
    </article>
  </NotificationPopover>;
}
