import { Search, X } from "lucide-react";
import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { IconButton } from "../components/Ui";
import { helpMarkdownComponents } from "./HelpMarkdown";
import { remarkErrorReference } from "./errorReference";

export function HelpErrorReference({ markdown }: { markdown: string }) {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  return <div className="help-error-reference">
    <div className="help-error-search">
      <Search aria-hidden />
      <input ref={inputRef} type="search" value={query} aria-label={t("helpCenter.errorSearch")}
        placeholder={t("helpCenter.errorSearch")} onChange={(event) => setQuery(event.target.value)} />
      {query ? <IconButton label={t("helpCenter.clearSearch")} icon={<X aria-hidden />} onClick={() => { setQuery(""); inputRef.current?.focus(); }} /> : null}
    </div>
    <ReactMarkdown key={query} skipHtml components={helpMarkdownComponents} remarkPlugins={[remarkGfm, [remarkErrorReference, {
      query, empty: t("helpCenter.noErrors"), results: (count: number) => t("helpCenter.errorResults", { count }),
    }]]}>{markdown}</ReactMarkdown>
  </div>;
}
