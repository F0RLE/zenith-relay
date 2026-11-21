import { useCallback, useEffect, useRef, useState } from "react";
import { ControlError } from "../../api/control";
import { customerNotices, markNoticesRead } from "../../api/notices";
import { Bell } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import { NotificationPopover } from "./NotificationPopover";

type Feed = Awaited<ReturnType<typeof customerNotices>>;

export function CustomerInbox({ csrf, languageOpen }: { csrf: string; languageOpen: boolean }) {
  const { t, locale } = useLocale();
  const [feed, setFeed] = useState<Feed | "loading" | "error">("loading");
  const [open, setOpen] = useState(false);
  const [reading, setReading] = useState(false);
  const anchor = useRef<HTMLButtonElement>(null);
  const active = useRef(false), marking = useRef(false);
  const pending = useRef<AbortController>(null);
  const close = useCallback(() => setOpen(false), []);
  const refresh = useCallback(() => {
    if (marking.current || !active.current) return;
    pending.current?.abort();
    const request = new AbortController();
    pending.current = request;
    setFeed(current => Array.isArray(current) ? current : "loading");
    void customerNotices(AbortSignal.any([request.signal, AbortSignal.timeout(12000)])).then(items => {
      if (!request.signal.aborted) setFeed(items);
    }, error => {
      if (request.signal.aborted) return;
      if (error instanceof ControlError && error.status === 401) location.replace("/login");
      else setFeed("error");
    });
  }, []);
  useEffect(() => {
    active.current = true;
    const visible = () => { if (!document.hidden) refresh(); };
    refresh();
    window.addEventListener("focus", visible);
    document.addEventListener("visibilitychange", visible);
    return () => {
      active.current = false; pending.current?.abort();
      window.removeEventListener("focus", visible);
      document.removeEventListener("visibilitychange", visible);
    };
  }, [refresh]);
  useEffect(() => { if (languageOpen) close(); }, [languageOpen, close]);

  const unread = Array.isArray(feed) ? feed.filter(n => !n.read).map(n => n.id) : [];
  async function mark() {
    if (unread.length === 0 || marking.current) return;
    marking.current = true; setReading(true);
    let reload = true;
    try {
      await markNoticesRead(csrf, unread);
    } catch (error) {
      if (active.current && error instanceof ControlError && error.status === 401) {
        setFeed("loading"); location.replace("/login");
        reload = false;
      } else if (!(error instanceof ControlError && error.status === 404)) {
        if (active.current) setFeed("error");
        reload = false;
      }
    } finally {
      marking.current = false;
      if (active.current) {
        setReading(false);
        if (reload) refresh();
      }
    }
  }
  const label = !Array.isArray(feed) ? t("shell.notifications") : unread.length > 0
    ? t("notifications.bellCount", { count: unread.length.toLocaleString(locale) }) : t("shell.notificationsNoNewNotifications");
  const date = new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" });
  return <>
    <button ref={anchor} type="button" className="icon-button notification-button"
      data-language-companion inert={languageOpen} aria-hidden={languageOpen || undefined}
      data-unread={unread.length > 0} aria-label={label} aria-haspopup="dialog" aria-expanded={open}
      aria-controls={open ? "notifications-panel" : undefined}
      onClick={() => { if (open) close(); else { setOpen(true); refresh(); } }}>
      <Bell aria-hidden="true" />
    </button>
    {open && !languageOpen && <NotificationPopover anchor={anchor} unread={unread.length} onRead={() => void mark()} reading={reading} onClose={close}>
      {feed === "loading" ? <p className="notification-feed-state" role="status">{t("unavailable.connecting")}</p>
        : feed === "error" ? <div className="notification-feed-state"><p role="alert">{t("notifications.loadFailed")}</p>
          <button type="button" className="text-button" onClick={() => { anchor.current?.focus({ preventScroll: true }); refresh(); }}>{t("common.retry")}</button></div>
          : feed.length === 0 ? <p className="notification-feed-state">{t("notifications.empty")}</p>
            : <ul className="notification-feed">{feed.map(n => <li key={n.id} className="notification-entry" data-unread={!n.read}>
              <span className="notification-event-icon"><Bell aria-hidden="true" /></span>
              <article className="notification-entry-content">
                <h3 dir="auto">{n.title}</h3>
                {!n.read && <span className="sr-only">{t("notifications.unread")}</span>}
                <p dir="auto">{n.message}</p>
                <time dateTime={new Date(n.publishedAt).toISOString()}>{date.format(n.publishedAt)}</time>
              </article>
            </li>)}</ul>}
    </NotificationPopover>}
  </>;
}
