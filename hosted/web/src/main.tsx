import { createRoot } from "react-dom/client";
import { lazy, Suspense, useEffect, useState } from "react";
import { fetchPublicConfig, type ClosedSite } from "./api/publicConfig";
import { OwnerSetup } from "./features/setup/OwnerSetup";
import { OwnerEntry, ownerPage } from "./features/owner/OwnerEntry";
import { PublicPage } from "./features/public/PublicPage";
import { CustomerEntry } from "./features/public/CustomerEntry";
import { customerPages } from "./features/app/routes";
import { LandingPage } from "./features/landing/LandingPage";
import { LanguageSwitcher } from "./components/LanguageSwitcher";
import { RelayMark } from "./components/RelayMark";
import { useTheme } from "./components/useTheme";
import { LocaleProvider, useLocale } from "./i18n/LocaleProvider";
import "./styles.css";
import "./features/app/entry.css";

const Preview = import.meta.env.DEV
  ? lazy(() => import("./preview/PreviewApp").then(module => ({ default: module.PreviewApp })))
  : null;
const publicPage = location.pathname === "/docs" ? "guide" : location.pathname === "/legal" ? "legal" : null;

function UnavailablePage({ site }: { site?: ClosedSite } = {}) {
  const { t } = useLocale();
  const { theme } = useTheme();
  return <div className="relay-app landing hosted-entry" data-theme={theme} style={{ colorScheme: theme }}>
    <header className="hosted-entry-header"><a className="landing-brand" href="/"><RelayMark className="landing-mark" /><strong>{site?.displayName ?? "Relay"}</strong></a><LanguageSwitcher /></header>
    <main className="integration-unavailable hosted-entry-content">
    {import.meta.env.DEV ? <><h1>Relay Hosted</h1><p>{t("unavailable.connection")}</p><p>{t("unavailable.details")}</p></>
      : <h1>{t(site ? "unavailable.setup" : "unavailable.server")}</h1>}
    <div className="hosted-entry-actions"><a className="key-button key-primary" href="/">{t("unavailable.home")}</a>
    {import.meta.env.DEV && <a className="key-button key-secondary" href="/preview.html">{t("unavailable.preview")}</a>}</div>
  </main></div>;
}

function Entry() {
  const { t } = useLocale();
  if (import.meta.env.PROD && location.pathname === "/setup/owner") return <OwnerSetup />;
  if (import.meta.env.PROD && location.pathname === "/setup/recovery") return <OwnerSetup recovery />;
  if (!import.meta.env.DEV) return <ServerEntry />;
  if (publicPage) return <PublicPage kind={publicPage} />;
  return location.pathname === "/" ? <LandingPage /> : Preview && /^\/(?:app|admin)(?:\/|$)/.test(location.pathname) && location.pathname !== "/admin/login" ? <Suspense fallback={<main className="integration-unavailable" role="status">{t("shell.openingDashboard")}</main>}><Preview /></Suspense> : <UnavailablePage />;
}

function ServerEntry() {
  const { t } = useLocale();
  const [site, setSite] = useState<ClosedSite | "loading" | null>("loading");
  useEffect(() => {
    const controller = new AbortController();
    let mounted = true;
    const timer = window.setTimeout(() => controller.abort(), 6000);
    void fetchPublicConfig(controller.signal)
      .then(config => { if (mounted) setSite(config); })
      .catch(() => { if (mounted) setSite(null); })
      .finally(() => window.clearTimeout(timer));
    return () => { mounted = false; window.clearTimeout(timer); controller.abort(); };
  }, []);
  useEffect(() => {
    if (site && site !== "loading") document.title = site.displayName;
  }, [site]);
  if (site === "loading") return <main className="integration-unavailable" aria-busy="true" role="status">{t("unavailable.connecting")}</main>;
  if (!site) return <UnavailablePage />;
  const owner = ownerPage(location.pathname);
  if (owner) return <OwnerEntry brand={site.displayName} page={owner} customerConfigured={!!site.loginProvider} />;
  const login = location.pathname === "/login";
  const customerPage = customerPages.find(p => location.pathname === `/app/${p.id}`);
  if (site.loginProvider && (customerPage || login || location.pathname === "/app"))
    return <CustomerEntry brand={site.displayName} provider={site.loginProvider} login={login}
      page={customerPage?.id ?? "overview"} support={site.policyLinks?.support} />;
  if (publicPage) return <PublicPage kind={publicPage} site={site} />;
  return location.pathname === "/" ? <LandingPage site={site} /> : <UnavailablePage site={site} />;
}

createRoot(document.getElementById("root")!).render(<LocaleProvider><Entry /></LocaleProvider>);
