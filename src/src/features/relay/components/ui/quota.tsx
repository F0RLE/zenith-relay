import { useTranslation } from "react-i18next";
import type { QuotaSnapshot, QuotaWindow } from "../../api/types";
import { formatDetailedRemainingTime, formatSupplementalQuotaLabel, isFastSupplementalQuota, quotaWindowLabel } from "../../quotaFormatting";

export function QuotaMeter({ window, kind, label, nowMs, concise = false }: { window: QuotaWindow | null; kind?: "primary" | "secondary"; label?: string; nowMs?: number; concise?: boolean }) {
  const { i18n, t } = useTranslation();
  const windowKind = kind ?? window?.kind ?? "primary";
  const resolvedLabel = label ?? quotaWindowLabel(window, windowKind, t);
  if (!window?.availableBasisPoints && window?.availableBasisPoints !== 0) {
    const unavailable = window ? t("common.unknown") : t("quota.notReported");
    return <div className="quota-meter unavailable"><div className="quota-meter-heading"><span>{resolvedLabel}</span><small>{unavailable}</small><strong>-</strong></div><div className="quota-track" aria-label={`${resolvedLabel}: ${unavailable}`} /></div>;
  }
  const percent = Math.round(window.availableBasisPoints / 100);
  const reset = window.resetAtMs
    ? nowMs == null
      ? new Intl.DateTimeFormat(i18n.language, { dateStyle: "short", timeStyle: "short" }).format(new Date(window.resetAtMs))
      : formatDetailedRemainingTime(window.resetAtMs, nowMs, t)
    : t("common.unknown");
  const resetLabel = concise ? reset : t("quota.reset", { value: reset });
  const remainingLabel = concise ? `${percent}%` : t("quota.remainingPercent", { value: percent });
  const level = percent <= 5 ? "critical" : percent <= 20 ? "low" : "normal";
  return <div className="quota-meter" data-level={level}><div className="quota-meter-heading"><span>{resolvedLabel}</span><small>{resetLabel}</small><strong>{remainingLabel}</strong></div><div className="quota-track" aria-label={`${resolvedLabel}: ${remainingLabel}`}><span style={{ width: `${percent}%` }} /></div></div>;
}

export function QuotaStack({ snapshot, nowMs, concise = false }: { snapshot: QuotaSnapshot; nowMs?: number; concise?: boolean }) {
  const { t } = useTranslation();
  // Fast/priority is a request-speed mode, not a second user-facing quota.
  // Keep the provider signal in the snapshot for diagnostics, but do not show
  // it beside the primary and feature-specific quota windows.
  const supplemental = (snapshot.supplemental ?? []).filter((item) => !isFastSupplementalQuota(item));
  const reported = [
    ...(["primary", "secondary"] as const).flatMap((kind) => {
      const window = snapshot[kind];
      if (!window) return [];
      return [{ id: kind, label: "", serviceTier: null, window }];
    }),
    ...supplemental,
  ];
  if (!reported.length) return <div className="quota-stack"><QuotaMeter window={null} concise={concise} {...(nowMs !== undefined ? { nowMs } : {})} /></div>;
  return <div className="quota-stack">{reported.map((item) => <QuotaMeter key={item.id} window={item.window} concise={concise} {...(item.label ? { label: `${formatSupplementalQuotaLabel(item.label, item.serviceTier, t)} · ${quotaWindowLabel(item.window, item.window.kind, t)}` } : {})} {...(nowMs !== undefined ? { nowMs } : {})} />)}</div>;
}

