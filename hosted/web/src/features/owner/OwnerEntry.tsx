import { lazy, Suspense, useEffect, useRef, useState } from "react";
import { OwnerApiError, readOwnerOverview, signInOwner, signOutOwner, supportsOwnerPasskey } from "../../api/ownerSession";
import { EntryFrame } from "../public/EntryFrame";
import { IdentityQualification } from "./IdentityQualification";
import { OwnerUsageOverview } from "./OwnerUsageOverview";
import { OwnerUsage } from "./OwnerUsage";
import { OwnerRequest } from "./OwnerRequest";
import { QualifiedPool } from "./QualifiedPool";
import { SavedConnections } from "./SavedConnections";
import { PairedDevices } from "./PairedDevices";
import { DesktopSourceCopy } from "./DesktopSourceCopy";
import { ServiceNotices } from "./ServiceNotices";
import { PaymentMethods } from "./PaymentMethods";
import { PaymentRefunds } from "./PaymentRefunds";
import { useLocale } from "../../i18n/LocaleProvider";

const OwnerOperations = lazy(() => import("./OwnerOperations").then(module => ({ default: module.OwnerOperations })));
const destinations = { overview: "nav.overview", connections: "nav.connections", pools: "nav.pool",
  "payment-methods": "paymentBuilder.title", usage: "nav.usage", notifications: "shell.notifications" } as const;
const ownerPages = Object.keys(destinations) as (keyof typeof destinations)[];
export function ownerPage(path: string) {
  return path === "/admin/login" ? "login" : path === "/admin/payments" ? "payments"
    : path === "/admin/operations" ? "operations" : /^\/admin\/usage\/requests\/[^/]+$/.test(path) ? "request"
    : ownerPages.find(p => path === `/admin/${p}`);
}

export function OwnerEntry({ brand, page, customerConfigured }: { brand: string; page: NonNullable<ReturnType<typeof ownerPage>>; customerConfigured: boolean }) {
  const { t } = useLocale();
  const login = page === "login";
  const [session, setSession] = useState<Awaited<ReturnType<typeof readOwnerOverview>>>();
  const [error, setError] = useState<"owner.loginFailed" | "owner.logoutFailed" | "unavailable.server" | "owner.loginBusy">();
  const [busy, setBusy] = useState(false);
  const [reload, setReload] = useState(0);
  const [connectionsVersion, setConnectionsVersion] = useState(0);
  const running = useRef(false);
  useEffect(() => {
    const restore = (e: PageTransitionEvent) => { if (e.persisted) location.reload(); };
    window.addEventListener("pageshow", restore);
    return () => window.removeEventListener("pageshow", restore);
  }, []);
  useEffect(() => {
    if (login) return;
    const controller = new AbortController();
    const timer = window.setTimeout(() => controller.abort(), 10000);
    let active = true;
    void readOwnerOverview(controller.signal).then(data => { if (active) setSession(data); }).catch(err => {
      if (!active) return;
      if (err instanceof OwnerApiError && err.status === 401) location.replace("/admin/login");
      else setError("unavailable.server");
    }).finally(() => window.clearTimeout(timer));
    return () => { active = false; controller.abort(); window.clearTimeout(timer); };
  }, [login, reload]);

  async function act() {
    if (running.current) return;
    running.current = true;
    setBusy(true);
    setError(undefined);
    try {
      if (login) {
        await signInOwner();
        location.replace("/admin/overview");
      } else if (session) {
        await signOutOwner(session.csrf);
        setSession(undefined);
        location.replace("/admin/login");
      } else setReload(n => n + 1);
    } catch (err) {
      if (!login && err instanceof OwnerApiError && err.status === 401) location.replace("/admin/login");
      else setError(err instanceof OwnerApiError && err.status === 429 ? "owner.loginBusy"
        : err instanceof OwnerApiError && err.status === 503 && login ? "unavailable.server"
        : login ? "owner.loginFailed" : "owner.logoutFailed");
    } finally { running.current = false; setBusy(false); }
  }

  const supported = !login || supportsOwnerPasskey();
  const usage = page === "usage" || page === "request";
  const operations = page === "operations";
  return <EntryFrame brand={brand} workspace={usage || operations || page === "overview" || page === "notifications" || page === "payment-methods"}>
      <h1 className={usage ? "owner-usage-heading" : operations ? "owner-operations-heading" : page === "overview" ? "owner-overview-heading" : page === "notifications" ? "owner-notices-heading" : page === "payment-methods" ? "payment-heading" : undefined}>
        {t(page === "login" ? "owner.login" : page === "payments" ? "refund.title" : page === "request" ? "ownerUsage.detail" : operations ? "operations.title" : destinations[page])}</h1>
      {login && <p>{t(supported ? "owner.loginHint" : "owner.passkeyUnsupported")}</p>}
      {session && <nav className="entry-links" aria-label={t("nav.owner")}>{ownerPages.map(p =>
        <a key={p} className="entry-account-link" href={"/admin/" + p} aria-current={p === page ? "page" : (p === "usage" && usage) || (p === "overview" && operations) ? "location" : undefined}>{t(destinations[p])}</a>)}</nav>}
      {session && page === "overview" && <>
        <div className="owner-overview-access">
          <p>{t(session.mode === "setup" ? "owner.closedMode" : session.mode === "qualification" ? "customer.restricted" : "customer.paused")}</p>
          <div className="owner-access-actions">
            <a className="entry-account-link entry-refunds-link" href="/admin/operations">{t("operations.title")}</a>
            <a className="entry-account-link entry-refunds-link" href="/admin/payments">{t("refund.title")}</a>
            {(customerConfigured || session.mode === "qualification") && <IdentityQualification onSession={setSession} />}
          </div>
        </div>
        <OwnerUsageOverview />
      </>}
      {session && page === "connections" && <>
        <SavedConnections key={connectionsVersion} session={session} onSession={setSession} />
        <PairedDevices session={session} onSession={setSession} />
        {location.protocol === "https:" && <DesktopSourceCopy session={session} onSession={setSession}
          onCommitted={() => setConnectionsVersion(n => n + 1)} />}
      </>}
      {session && page === "pools" && <QualifiedPool session={session} onSession={setSession} />}
      {session && page === "notifications" && <ServiceNotices session={session} onSession={setSession} />}
      {session && page === "payment-methods" && <PaymentMethods session={session} onSession={setSession} />}
      {session && page === "payments" && <PaymentRefunds session={session} onSession={setSession} />}
      {session && page === "usage" && <OwnerUsage />}
      {session && page === "request" && <OwnerRequest />}
      {session && operations && <Suspense fallback={<p role="status">{t("unavailable.connecting")}</p>}>
        <OwnerOperations session={session} onSession={setSession} /></Suspense>}
      {!login && !session && !error && <p role="status">{t("unavailable.connecting")}</p>}
      {error && <p role="alert">{t(error)}</p>}
      {supported && (login || session || error) && <button className={`key-button ${login ? "key-primary" : "key-secondary"}`}
        disabled={busy} onClick={() => void act()}>{t(login ? "owner.signInPasskey" : session ? "shell.signOut" : "common.retry")}</button>}
  </EntryFrame>;
}
