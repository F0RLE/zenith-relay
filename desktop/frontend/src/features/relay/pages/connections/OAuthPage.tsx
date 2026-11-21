import { lazy, Suspense, useLayoutEffect, useRef, useState } from "react";
import { CircleAlert, Loader2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { OAuthPageState, OAuthViewBounds } from "../../api/types";
const OAuthEditMenu = lazy(async () => ({ default: (await import("./OAuthEditMenu")).OAuthEditMenu }));

let viewRevision = 0;

export function OAuthPage({ loginId, pageState, suspended, automatic, unavailable, onReady }: {
  loginId: string;
  pageState: OAuthPageState;
  suspended: boolean;
  automatic: boolean;
  unavailable: boolean;
  onReady: (loginId: string | null) => void;
}) {
  const { t } = useTranslation();
  const slot = useRef<HTMLDivElement>(null);
  const [status, setStatus] = useState<"opening" | "ready" | "failed">("opening");
  const [menuReady, setMenuReady] = useState<string | null>(null);
  useLayoutEffect(() => {
    const element = slot.current;
    if (!element || menuReady !== loginId || pageState !== "opened" || suspended || automatic || unavailable) return;
    let frame = 0;
    let disposed = false;
    let lastGeometry = "";
    let placement = 0;
    const dialog = element.closest("[data-relay-dialog]");
    const hidden = () => document.hidden
      || dialog !== [...document.querySelectorAll("[data-relay-dialog]")].at(-1)
      || Boolean(document.querySelector(".app-context-menu"));
    let covered = hidden();
    const place = () => {
      const bounds = covered ? null : measure(element);
      const key = JSON.stringify(bounds);
      if (key === lastGeometry) return;
      lastGeometry = key;
      const revision = placement = ++viewRevision;
      void relayCommands.placeOAuthView(loginId, revision, bounds).then((placed) => {
        if (disposed || placement !== revision || !bounds) return;
        onReady(placed ? loginId : null);
        setStatus(placed ? "ready" : "failed");
      }).catch(() => {
        if (disposed || placement !== revision || !bounds) return;
        onReady(null);
        setStatus("failed");
      });
    };
    const schedule = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(place);
    };
    const visibilityChanged = () => {
      const next = hidden();
      if (covered !== next) {
        covered = next;
        place();
      }
    };
    const sizes = new ResizeObserver(schedule);
    sizes.observe(element);
    sizes.observe(document.documentElement);
    const overlays = new MutationObserver(visibilityChanged);
    overlays.observe(document.body, { childList: true, subtree: true });
    let scale = matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
    const scaleChanged = () => {
      scale.removeEventListener("change", scaleChanged);
      scale = matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
      scale.addEventListener("change", scaleChanged);
      schedule();
    };
    scale.addEventListener("change", scaleChanged);
    document.addEventListener("visibilitychange", visibilityChanged);
    window.addEventListener("resize", schedule);
    window.addEventListener("scroll", schedule, true);
    place();
    return () => {
      disposed = true;
      sizes.disconnect();
      overlays.disconnect();
      scale.removeEventListener("change", scaleChanged);
      document.removeEventListener("visibilitychange", visibilityChanged);
      window.removeEventListener("resize", schedule);
      window.removeEventListener("scroll", schedule, true);
      cancelAnimationFrame(frame);
      void relayCommands.placeOAuthView(loginId, ++viewRevision, null).catch(() => undefined);
    };
  }, [automatic, loginId, menuReady, onReady, pageState, suspended, unavailable]);

  const state = pageState === "opened" ? status : pageState;
  return <><section className="oauth-page" aria-label={t("accounts.signInPage")}>
    <div className="oauth-page-slot" ref={slot}>
      {unavailable || state === "ready" ? null : <div className="oauth-page-state" role={state === "failed" ? "alert" : state === "opening" ? "status" : undefined}>
        {state === "failed" ? <CircleAlert aria-hidden /> : state === "opening" ? <Loader2 className="spin" aria-hidden /> : null}
        <span>{t(state === "closed" ? "accounts.signInPageClosed" : state === "failed" ? "accounts.signInPageUnavailable" : automatic ? "accounts.automaticSignInProgress" : "accounts.preparingSignIn")}</span>
      </div>}
    </div>
  </section>{pageState === "opened" && !suspended && !automatic && !unavailable ? <Suspense fallback={null}><OAuthEditMenu loginId={loginId} active slot={slot} onReady={setMenuReady} /></Suspense> : null}</>;
}

function measure(element: HTMLElement): OAuthViewBounds | null {
  const { left, top, right, bottom } = element.getBoundingClientRect();
  const ratio = window.devicePixelRatio;
  const x = Math.ceil(Math.max(0, left) * ratio);
  const y = Math.ceil(Math.max(0, top) * ratio);
  const width = Math.floor(Math.min(window.innerWidth, right) * ratio) - x;
  const height = Math.floor(Math.min(window.innerHeight, bottom) * ratio) - y;
  return width > 0 && height > 0 ? { x, y, width, height } : null;
}
