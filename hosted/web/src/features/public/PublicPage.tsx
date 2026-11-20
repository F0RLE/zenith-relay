import { useTheme } from "../../components/useTheme";
import { policyKinds, type ClosedSite } from "../../api/publicConfig";
import { useLocale } from "../../i18n/LocaleProvider";
import { PublicHeader } from "../landing/PublicHeader";
import "./public.css";

const steps = ["model", "key", "client", "usage"] as const;

export function PublicPage({ kind, site }: { kind: "guide" | "legal"; site?: ClosedSite }) {
  const { t } = useLocale();
  const { theme, toggleTheme } = useTheme();
  const legal = kind === "legal";
  const links = policyKinds.flatMap(key => site?.policyLinks?.[key] ? [{ key, href: site.policyLinks[key] }] : []);
  return <div className="relay-app landing landing-home public-reading" data-theme={theme} style={{ colorScheme: theme }}>
    <a className="skip-link" href="#main-content">{t("landing.skip")}</a>
    <PublicHeader brand={site?.displayName ?? "Relay"} theme={theme} toggleTheme={toggleTheme} />
    <main id="main-content" className="public-reader">
      <nav aria-label={t("public.pages")} className="public-nav">
        <a href="/docs" aria-current={!legal ? "page" : undefined}>{t("public.guide")}</a>
        <a href="/legal" aria-current={legal ? "page" : undefined}>{t("public.legal")}</a>
      </nav>
      <h1>{t(legal ? "public.legal" : "public.guide")}</h1>
      {legal ? links.length ? <ul className="public-policy-links">{links.map(({ key, href }) =>
        <li key={key}><a href={href} rel="noreferrer">{t(`public.policy.${key}`)}<span aria-hidden="true">↗</span></a></li>)}</ul>
        : <p>{t("public.noPolicies")}</p>
        : <><p>{t("public.guideLead")}</p>
          <ol className="public-steps">{steps.map(step => <li key={step}>
            <h2>{t(`public.step.${step}`)}</h2><p>{t(`public.step.${step}.body`)}</p>
          </li>)}</ol></>}
    </main>
  </div>;
}
