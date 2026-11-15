import { useState } from "react";
import { ArrowUp, ArrowUpRight } from "../../components/Icons";
import { useTheme } from "../../components/useTheme";
import { ThemeIcon } from "../../components/ThemeIcon";
import { LandingLanguageSwitcher } from "../../components/LandingLanguageSwitcher";
import { RelayMark } from "../../components/RelayMark";
import { useLocale } from "../../i18n/LocaleProvider";
import { RequestWeave } from "./RequestWeave";
import "./landing.css";

const questions = [
  ["faq.what.question", "faq.what.answer"],
  ["faq.chat.question", "faq.chat.answer"],
  ["faq.price.question", "faq.price.answer"],
  ["faq.start.question", "faq.start.answer"],
] as const;

export function LandingPage() {
  const { theme, toggleTheme } = useTheme();
  const { t } = useLocale();
  const [languageOpen, setLanguageOpen] = useState(false);
  const startHref = "/app/overview";
  const cabinetHref = "/app/overview";
  return <div className="relay-app landing landing-home" data-theme={theme} style={{ colorScheme: theme }}>
    <a className="skip-link" href="#main-content">{t("landing.skip")}</a>
    <header className="landing-header"><div className="landing-container landing-header-inner">
      <a className="landing-brand" href="/" aria-label={t("landing.home")}><RelayMark className="landing-mark" /><strong>Relay</strong></a>
      <div className="landing-header-actions">
        <LandingLanguageSwitcher open={languageOpen} onOpenChange={setLanguageOpen} />
        <button type="button" className="key-button key-secondary key-icon" data-theme-toggle aria-label={t(theme === "light" ? "theme.dark" : "theme.light")} onClick={toggleTheme}><ThemeIcon theme={theme} /></button>
        <a className="key-button key-secondary" href={cabinetHref}>{t("landing.cabinet")}</a>
      </div>
    </div></header>
    <main id="main-content" className="landing-main">
      <section className="landing-hero" id="top" aria-labelledby="landing-title">
        <div className="landing-hero-content"><div className="landing-copy">
          <h1 id="landing-title"><span>{t("landing.title")}</span>{" "}<span>{t("landing.titleEnd")}</span></h1>
          <p>{t("landing.description")}</p>
          <div className="landing-actions"><a className="key-button key-primary landing-start" href={startHref}>{t("landing.start")}<ArrowUpRight aria-hidden="true" /></a><a className="landing-catalog-link" href="/app/models">{t("landing.modelsAndPricing")}</a></div>
        </div></div>
        <RequestWeave description={t("flow.description")} inputLabel={t("flow.input")} outputLabel={t("flow.output")} />
      </section>
      <section className="landing-faq landing-container" id="features" aria-labelledby="faq-title">
        <div className="landing-section-heading" id="faq"><h2 id="faq-title">{t("landing.questions")}</h2><svg className="faq-weave-mark" viewBox="0 0 160 160" fill="none" aria-hidden="true">{Array.from({ length: 9 }, (_, index) => <path key={index} d={`M${10 + index * 7} 0v40c0 40 ${84 - index * 7} 20 ${84 - index * 7} 60v60M160 ${10 + index * 7}h-40c-40 0-20 ${84 - index * 7}-60 ${84 - index * 7}H0`} />)}</svg></div>
        <div className="landing-questions" id="how-it-works">
          {questions.map(([question, answer], index) => <article key={question} aria-labelledby={`question-${index}`}><h3 id={`question-${index}`}>{t(question)}</h3><p>{t(answer)}</p></article>)}
          <a className="key-button key-primary faq-start" href={startHref}>{t("landing.openCabinet")}<ArrowUpRight aria-hidden="true" /></a>
        </div>
      </section>
    </main>
    <footer className="landing-footer"><div className="landing-container footer-bottom"><a className="landing-brand" href="/"><strong>Relay</strong><span>Hosted</span></a><a className="landing-top-link" href="#top">{t("landing.top")}<ArrowUp aria-hidden="true" /></a></div></footer>
  </div>;
}
