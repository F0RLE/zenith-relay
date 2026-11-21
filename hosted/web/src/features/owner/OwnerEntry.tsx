import { lazy, Suspense, useEffect, useRef, useState } from "react";
import type { ComponentProps } from "react";
import { OwnerApiError, readOwnerOverview, signInOwner, signOutOwner, supportsOwnerPasskey } from "../../api/ownerSession";
import { useTheme } from "../../components/useTheme";
import { HostedShell } from "../app/HostedShell";
import type { HostedNavItem } from "../app/HostedShell";
import { EntryFrame } from "../public/EntryFrame";
import { IdentityQualification } from "./IdentityQualification";
import { OwnerUsageOverview } from "./OwnerUsageOverview";
import { OwnerUsage } from "./OwnerUsage";
import { OwnerRequest } from "./OwnerRequest";
import { SavedConnections } from "./SavedConnections";
import { ServiceNotices } from "./ServiceNotices";
import { PaymentMethods } from "./PaymentMethods";
import { PaymentRefunds } from "./PaymentRefunds";
import { useLocale } from "../../i18n/LocaleProvider";

const OwnerOperations = lazy(() => import("./OwnerOperations").then(module => ({ default: module.OwnerOperations })));
const QualifiedPool = lazy(() => import("./QualifiedPool").then(module => ({ default: module.QualifiedPool })));
const OwnerPasskeys = lazy(() => import("./OwnerPasskeys").then(module => ({ default: module.OwnerPasskeys })));
const OwnerCustomerCase = lazy(() => import("./OwnerCustomerCase").then(module => ({ default: module.OwnerCustomerCase })));
const destinations = { overview: "nav.overview", connections: "nav.connections", pools: "nav.pool",
  "payment-methods": "paymentBuilder.title", usage: "nav.usage", notifications: "shell.notifications" } as const;
const ownerPages = Object.keys(destinations) as (keyof typeof destinations)[];
export function ownerPage(path: string) {
  return path === "/admin/login" ? "login" : path === "/admin/payments" ? "payments"
    : path === "/admin/operations" ? "operations" : /^\/admin\/usage\/requests\/[^/]+$/.test(path) ? "request"
    : /^\/admin\/customers\/[^/]+$/.test(path) ? "customer"
    : ownerPages.find(p => path === `/admin/${p}`);
}

function OwnerShell(props: Omit<ComponentProps<typeof HostedShell>, "appearance" | "serverWorkspace">) {
  const appearance = useTheme();
  return <HostedShell {...props} appearance={appearance} serverWorkspace="owner" />;
}

export function OwnerEntry({ brand, page, customerConfigured }: { brand: string; page: NonNullable<ReturnType<typeof ownerPage>>; customerConfigured: boolean }) {
  const { t } = useLocale();
  const login = page === "login";
  const [session, setSession] = useState<Awaited<ReturnType<typeof readOwnerOverview>>>();
  const [error, setError] = useState<"owner.loginFailed" | "owner.logoutFailed" | "unavailable.server" | "owner.loginBusy">();
  const [busy, setBusy] = useState(false);
  const [reload, setReload] = useState(0);
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
  const usage = page === "usage" || page === "request" || page === "customer";
  const operations = page === "operations";
  const title = t(page === "login" ? "owner.login" : page === "payments" ? "refund.title" : page === "request" ? "ownerUsage.detail"
    : page === "customer" ? "access.case" : operations ? "operations.title" : destinations[page]);
  if (login || !session) return <EntryFrame brand={brand} workspace={usage || operations || page === "overview" || page === "notifications" || page === "payment-methods"}>
    <h1>{title}</h1>
    {login && <p>{t(supported ? "owner.loginHint" : "owner.passkeyUnsupported")}</p>}
    {!login && !error && <p role="status">{t("unavailable.connecting")}</p>}
    {error && <p role="alert">{t(error)}</p>}
    {supported && (login || error) && <button className={`key-button ${login ? "key-primary" : "key-secondary"}`}
      disabled={busy} onClick={() => void act()}>{t(login ? "owner.signInPasskey" : "common.retry")}</button>}
  </EntryFrame>;
  const navigation = ownerPages.map((id): HostedNavItem => ({ id, name: destinations[id], href: `/admin/${id}`,
    current: id === page ? "page" : (id === "usage" && usage) || (id === "overview" && (operations || page === "payments")) ? "location" : undefined }));
  return <OwnerShell brand={brand} page={page} navigation={navigation} onSignOut={() => void act()} signingOut={busy}>
      {page !== "connections" && page !== "pools" && <header className="page-heading"><h1>{title}</h1></header>}
      {error && <p role="alert">{t(error)}</p>}
      {session && page === "overview" && <>
        <div className="owner-overview-access">
          <p>{t(session.mode === "setup" ? "owner.closedMode" : session.mode === "qualification" ? "customer.restricted" : "customer.paused")}</p>
          <div className="owner-access-actions">
            <a className="entry-account-link entry-refunds-link" href="/admin/operations">{t("operations.title")}</a>
            <a className="entry-account-link entry-refunds-link" href="/admin/payments">{t("refund.title")}</a>
            {(customerConfigured || session.mode === "qualification") && <IdentityQualification csrf={session.csrf} onSession={setSession} />}
          </div>
        </div>
        <Suspense fallback={null}><OwnerPasskeys key={session.csrf} csrf={session.csrf} setup={session.mode === "setup"} /></Suspense>
        <OwnerUsageOverview />
      </>}
      {page === "connections" && <SavedConnections session={session} />}
      {page === "pools" && <Suspense fallback={<p role="status">{t("unavailable.connecting")}</p>}>
        <QualifiedPool session={session} />
      </Suspense>}
      {session && page === "notifications" && <ServiceNotices session={session} />}
      {session && page === "payment-methods" && <PaymentMethods session={session} />}
      {session && page === "payments" && <PaymentRefunds session={session} />}
      {session && page === "usage" && <OwnerUsage />}
      {session && page === "request" && <OwnerRequest />}
      {session && page === "customer" && <Suspense fallback={<p role="status">{t("unavailable.connecting")}</p>}>
        <OwnerCustomerCase csrf={session.csrf} /></Suspense>}
      {session && operations && <Suspense fallback={<p role="status">{t("unavailable.connecting")}</p>}>
        <OwnerOperations session={session} /></Suspense>}
  </OwnerShell>;
}
