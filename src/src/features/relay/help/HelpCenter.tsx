import { RotateCcw } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import enGuide from "../../../../../docs/help/en/README.md?url";
import ruGuide from "../../../../../docs/help/ru/README.md?url";
import { Button, Dialog, PageHeader } from "../components/Ui";
import { useRelayState } from "../state/RelayStateProvider";
import { HelpContents } from "./HelpContents";
import { HelpErrorReference } from "./HelpErrorReference";
import { helpMarkdownComponents } from "./HelpMarkdown";

const guides = {
  en: enGuide,
  ru: ruGuide,
} satisfies Record<"en" | "ru", string>;

export function HelpCenter() {
  const { t, i18n } = useTranslation();
  const { resetOnboarding } = useRelayState();
  const language = i18n.resolvedLanguage?.startsWith("ru") ? "ru" : "en";

  return <section className="relay-page help-page">
    <PageHeader title={t("common.help")} actions={<Button icon={<RotateCcw aria-hidden />} onClick={resetOnboarding}>{t("helpCenter.quickSetup")}</Button>} />
    <HelpGuide key={language} language={language} />
  </section>;
}

export function HelpTopicDialog({ topic, onClose }: { topic: "sign-in"; onClose: () => void }) {
  const { t, i18n } = useTranslation();
  const language = i18n.resolvedLanguage?.startsWith("ru") ? "ru" : "en";
  return <Dialog
    title={t("accounts.signInHelp")}
    onClose={onClose}
    layer="top"
    className="help-topic-dialog"
    footer={<Button variant="primary" onClick={onClose}>{t("common.close")}</Button>}
  >
    <HelpGuide key={`${language}:${topic}`} language={language} topic={topic} />
  </Dialog>;
}

function HelpGuide({ language, topic }: { language: keyof typeof guides; topic?: "sign-in" }) {
  const { t } = useTranslation();
  const [markdown, setMarkdown] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    const abort = new AbortController();
    setFailed(false);
    void fetch(guides[language], { signal: abort.signal }).then(async (response) => {
      if (!response.ok) throw new Error("Help asset unavailable");
      const text = await response.text();
      if (!text.includes("<!-- relay:error-reference -->")) throw new Error("Invalid Help asset");
      const content = topic
        ? text.split(`<!-- relay:topic:${topic} -->`)[1]?.split(`<!-- relay:topic-end:${topic} -->`)[0]?.trim()
        : text;
      if (!content || (topic && !text.includes(`<!-- relay:topic-end:${topic} -->`))) throw new Error("Help topic unavailable");
      if (!abort.signal.aborted) setMarkdown(content);
    }).catch(() => { if (!abort.signal.aborted) setFailed(true); });
    return () => abort.abort();
  }, [language, topic, attempt]);
  if (failed) return <div role="alert"><p>{t("helpCenter.loadFailed")}</p><Button icon={<RotateCcw aria-hidden />} onClick={() => setAttempt((attempt) => attempt + 1)}>{t("helpCenter.retry")}</Button></div>;
  if (markdown === null) return <p role="status">{t("helpCenter.loading")}</p>;
  if (topic) return <article className="help-document">
    <ReactMarkdown skipHtml remarkPlugins={[remarkGfm]} components={helpMarkdownComponents}>{markdown}</ReactMarkdown>
  </article>;
  return <HelpDocument markdown={markdown} language={language} />;
}

function HelpDocument({ markdown, language }: { markdown: string; language: string }) {
  const documentRef = useRef<HTMLElement>(null);
  const [guide, reference] = markdown.split("<!-- relay:error-reference -->");
  return <div className="help-layout">
    <article ref={documentRef} className="help-document">
      <ReactMarkdown skipHtml remarkPlugins={[remarkGfm]} components={helpMarkdownComponents}>{guide}</ReactMarkdown>
      {reference ? <HelpErrorReference markdown={reference} /> : null}
    </article>
    <HelpContents documentRef={documentRef} language={language} />
  </div>;
}
