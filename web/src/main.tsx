import { createRoot } from "react-dom/client";
import { lazy, Suspense } from "react";
import { LandingPage } from "./features/landing/LandingPage";
import { LanguageSwitcher } from "./components/LanguageSwitcher";
import { RelayMark } from "./components/RelayMark";
import { useTheme } from "./components/useTheme";
import { LocaleProvider, useLocale } from "./i18n/LocaleProvider";
import "./styles.css";
import "./features/app/entry.css";

const HostedEntry = lazy(() => import.meta.env.DEV
  ? import("./preview/PreviewApp").then(module => ({ default: module.PreviewApp }))
  : import("./features/app/HostedApp").then(module => ({ default: module.HostedApp })));

function UnavailablePage() {
  const { t } = useLocale();
  const { theme } = useTheme();
  return <div className="relay-app landing hosted-entry" data-theme={theme} style={{ colorScheme: theme }}>
    <header className="hosted-entry-header"><a className="landing-brand" href="/" aria-label={t("landing.home")}><RelayMark className="landing-mark" /><strong>Relay</strong></a><LanguageSwitcher /></header>
    <main className="integration-unavailable hosted-entry-content">
    <h1>Relay Hosted</h1>
    <p>{t("unavailable.connection")}</p>
    <p>{t("unavailable.details")}</p>
    <div className="hosted-entry-actions"><a className="key-button key-primary" href="/">{t("unavailable.home")}</a>
    {import.meta.env.DEV && <a className="key-button key-secondary" href="/preview.html">{t("unavailable.preview")}</a>}</div>
  </main></div>;
}

function Entry() {
  const { t } = useLocale();
  return location.pathname === "/" ? <LandingPage /> : /^\/(?:app|admin)(?:\/|$)/.test(location.pathname) && location.pathname !== "/admin/login" ? <Suspense fallback={<main className="integration-unavailable" role="status">{t("shell.openingDashboard")}</main>}><HostedEntry /></Suspense> : <UnavailablePage />;
}

createRoot(document.getElementById("root")!).render(<LocaleProvider><Entry /></LocaleProvider>);
