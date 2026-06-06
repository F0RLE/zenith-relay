import { Clock, ChevronLeft, ChevronRight } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { UsageLogEntry } from "../tauri";

type HistoryPanelProps = {
  entries: UsageLogEntry[];
  error: boolean;
  loading: boolean;
  canLoadMore: boolean;
  onLoadLatest: () => void;
  onLoadMore: () => void;
};

export function HistoryPanel({
  entries,
  error,
  loading,
  canLoadMore,
  onLoadLatest,
  onLoadMore,
}: HistoryPanelProps) {
  const { t } = useTranslation();

  if (error) {
    return (
      <section className="history-panel empty-panel">
        <Clock aria-hidden />
        <span>{t("history.failed")}</span>
      </section>
    );
  }

  if (!loading && entries.length === 0) {
    return (
      <section className="history-panel empty-panel">
        <Clock aria-hidden />
        <span>{t("history.empty")}</span>
      </section>
    );
  }

  return (
    <section className="history-panel" aria-label={t("history.label")}>
      <div className="history-list">
        {entries.map((entry) => (
          <article className="history-row" key={entry.id}>
            <div>
              <strong>{entry.modelDisplay || t("history.unknownModel")}</strong>
              <span>{entry.createdAtDisplay || entry.createdAt}</span>
            </div>
            <div>
              <strong>{entry.cost}</strong>
              <span>{entry.totalTokensDisplay} {t("history.tokens")}</span>
            </div>
            <div>
              <span>{t("stats.inputTokens")}: {entry.inputTokensDisplay}</span>
              <span>{t("stats.outputTokens")}: {entry.outputTokensDisplay}</span>
            </div>
            <div>
              <span>{t("stats.reasoningTokens")}: {entry.reasoningTokensDisplay}</span>
              <span>{t("stats.cachedTokens")}: {entry.cachedInputTokensDisplay}</span>
            </div>
          </article>
        ))}
      </div>
      <div className="history-actions">
        <button type="button" onClick={onLoadLatest} disabled={loading} title={t("history.latest")}>
          <ChevronLeft aria-hidden />
          <span>{t("history.latest")}</span>
        </button>
        <button type="button" onClick={onLoadMore} disabled={loading || !canLoadMore} title={t("history.more")}>
          <span>{loading ? t("history.loading") : t("history.more")}</span>
          <ChevronRight aria-hidden />
        </button>
      </div>
    </section>
  );
}
