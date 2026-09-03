import { RotateCcw } from "lucide-react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import enGuide from "../../../../../docs/help/en/README.md?raw";
import ruGuide from "../../../../../docs/help/ru/README.md?raw";
import { Button, PageHeader } from "../components/Ui";
import { useRelayState } from "../state/RelayStateProvider";

const guides = {
  en: enGuide,
  ru: ruGuide,
} satisfies Record<"en" | "ru", string>;
const markdownComponents: Components = {
  h1: ({ children }) => <h1 id={headingId(children)}>{children}</h1>,
  h2: ({ children }) => <h2 id={headingId(children)} tabIndex={-1}>{children}</h2>,
  h3: ({ children }) => <h3 id={headingId(children)} tabIndex={-1}>{children}</h3>,
  table: ({ children }) => <div className="help-table-wrap"><table>{children}</table></div>,
};

export function HelpCenter() {
  const { t, i18n } = useTranslation();
  const { resetOnboarding } = useRelayState();
  const language = i18n.resolvedLanguage?.startsWith("ru") ? "ru" : "en";

  return <section className="relay-page help-page">
    <PageHeader title={t("common.help")} subtitle={t("helpCenter.subtitle")} actions={<Button icon={<RotateCcw aria-hidden />} onClick={resetOnboarding}>{t("helpCenter.quickSetup")}</Button>} />
    <article className="help-document"><ReactMarkdown skipHtml remarkPlugins={[remarkGfm]} components={markdownComponents}>{guides[language]}</ReactMarkdown></article>
  </section>;
}

function headingId(children: ReactNode) {
  return String(children).toLocaleLowerCase().trim().replace(/[^\p{L}\p{N}]+/gu, "-").replace(/^-|-$/g, "");
}
