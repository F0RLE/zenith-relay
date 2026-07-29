import { RotateCcw } from "lucide-react";
import type { ReactNode } from "react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import enLocal from "../../../../../docs/help/en/this-computer.md?raw";
import enRemote from "../../../../../docs/help/en/my-server.md?raw";
import enZenith from "../../../../../docs/help/en/zenith-api.md?raw";
import ruLocal from "../../../../../docs/help/ru/this-computer.md?raw";
import ruRemote from "../../../../../docs/help/ru/my-server.md?raw";
import ruZenith from "../../../../../docs/help/ru/zenith-api.md?raw";
import type { RelayMode } from "../api/types";
import { Button, PageHeader, Tabs } from "../components/Ui";
import { useRelayState } from "../state/RelayStateProvider";

const modes: RelayMode[] = ["local", "zenith", "remote"];
const guides = {
  en: { local: enLocal, zenith: enZenith, remote: enRemote },
  ru: { local: ruLocal, zenith: ruZenith, remote: ruRemote },
} satisfies Record<"en" | "ru", Record<RelayMode, string>>;
const markdownComponents: Components = {
  h1: ({ children }) => <h1 id={headingId(children)}>{children}</h1>,
  h2: ({ children }) => <h2 id={headingId(children)} tabIndex={-1}>{children}</h2>,
  h3: ({ children }) => <h3 id={headingId(children)} tabIndex={-1}>{children}</h3>,
};

export function HelpCenter() {
  const { t, i18n } = useTranslation();
  const { mode, resetOnboarding } = useRelayState();
  const [selectedMode, setSelectedMode] = useState<RelayMode>(mode);
  const language = i18n.resolvedLanguage?.startsWith("ru") ? "ru" : "en";

  return <section className="relay-page help-page">
    <PageHeader title={t("common.help")} subtitle={t("helpCenter.subtitle")} actions={<Button icon={<RotateCcw aria-hidden />} onClick={resetOnboarding}>{t("helpCenter.quickSetup")}</Button>} />
    <Tabs label={t("helpCenter.modeLabel")} value={selectedMode} onChange={(value) => setSelectedMode(value as RelayMode)} items={modes.map((value) => ({ id: value, label: t(`helpCenter.modes.${value}.title`) }))} />
    <article className="help-document"><ReactMarkdown skipHtml remarkPlugins={[remarkGfm]} components={markdownComponents}>{guides[language][selectedMode]}</ReactMarkdown></article>
  </section>;
}

function headingId(children: ReactNode) {
  return String(children).toLocaleLowerCase().trim().replace(/[^\p{L}\p{N}]+/gu, "-").replace(/^-|-$/g, "");
}
