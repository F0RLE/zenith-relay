import { useTranslation } from "react-i18next";
import type { KeyStats } from "../tauri";

type StatsGridProps = {
  keyStats: KeyStats | null;
};

export function StatsGrid({ keyStats }: StatsGridProps) {
  const { t } = useTranslation();

  return (
    <div className="stats-grid" aria-label={t("stats.label")}>
      <Stat label={t("stats.balance")} value={keyStats?.balance ?? "$0.00"} tone="positive" />
      <Stat label={t("stats.spent")} value={keyStats?.spent ?? "$0.00"} tone="money" />
      <Stat label={t("stats.requests")} value={keyStats?.requestsDisplay ?? "0"} />
      <Stat label={t("stats.totalTokens")} value={keyStats?.totalTokensDisplay ?? "0"} />
      <Stat label={t("stats.inputTokens")} value={keyStats?.inputTokensDisplay ?? "0"} />
      <Stat label={t("stats.reasoningTokens")} value={keyStats?.reasoningTokensDisplay ?? "0"} />
      <Stat label={t("stats.outputTokens")} value={keyStats?.outputTokensDisplay ?? "0"} />
      <Stat label={t("stats.cachedTokens")} value={keyStats?.cachedInputTokensDisplay ?? "0"} />
      <Stat label={t("stats.month")} value={keyStats?.monthlySpent ?? "$0.00"} />
    </div>
  );
}

function Stat({
  label,
  value,
  tone,
}: {
  label: string;
  value: string;
  tone?: "positive" | "money";
}) {
  return (
    <div className={`stat ${tone ? `tone-${tone}` : ""}`}>
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}
