import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button, CopyButton, EmptyState, StatusBadge } from "../../components/Ui";
import type { HostedDevicePage, HostedPoolChange, HostedPoolReview } from "../../api/types/hostedDevice";
import { useHostedFormatting } from "./HostedObservations";

function ReviewCode({ review, focus }: { review: HostedPoolReview; focus: boolean }) {
  const { t } = useTranslation();
  const { date } = useHostedFormatting();
  const ref = useRef<HTMLInputElement>(null);
  const [expired, setExpired] = useState(false);
  const expiry = review.expires_at_s * 1000;
  useEffect(() => {
    const check = () => setExpired(Date.now() >= expiry);
    const timer = window.setTimeout(check, Math.max(0, expiry - Date.now()));
    document.addEventListener("visibilitychange", check);
    return () => { window.clearTimeout(timer); document.removeEventListener("visibilitychange", check); };
  }, [expiry]);
  useEffect(() => { if (focus) ref.current?.focus(); }, [focus]);
  if (expired || Date.now() >= expiry) return <small role="status">{t("hostedPool.expired")}</small>;
  return <div className="hosted-pool-review">
    <label className="relay-field">
      <span>{t("hostedPool.paste")}</span>
      <input readOnly ref={ref} value={review.operation_id} onFocus={e => e.target.select()} />
    </label>
    <div className="hosted-device-actions">
      <CopyButton value={review.operation_id} label={t("common.copy")} />
      <small>{t("hostedPool.expires", { date: date(expiry) })}</small>
    </div>
  </div>;
}

export function HostedConnections({ page, poolView, busy, focusReview, onReview }: {
  page: HostedDevicePage; poolView: boolean; busy: boolean; focusReview?: string | undefined; onReview: (cmd: HostedPoolChange) => void;
}) {
  const { t } = useTranslation();
  const rows = poolView ? page.items.filter(({ pool }) => pool && (pool.in_pool || pool.draining || pool.action === "include")) : page.items;
  const review = poolView ? page.pool_review : undefined;
  const pending = review && <ReviewCode key={review.operation_id} review={review} focus={focusReview === review.operation_id} />;
  const offPage = review && !rows.some(row => row.id === review.source_id);
  return <>
    {offPage && pending}
    {rows.length === 0 ? <EmptyState title={t(poolView ? "hostedPool.empty" : "hosted.empty")} /> : <ul className="hosted-source-list">
      {rows.map(row => {
        const pool = row.pool;
        const status = pool?.draining ? "draining" : pool?.state === "trashed" ? "trashed" :
          pool?.in_pool ? (pool.state === "paused" ? "paused" : "member") : "outside";
        return <li key={row.id}>
          <div className="hosted-source-head">
            <strong>{row.label ?? row.id}</strong>
            {pool && <StatusBadge status={status === "member" ? "ready" :
              status === "draining" || status === "paused" ? "warning" : "disabled"} label={t(`hostedPool.${status}`)} />}
          </div>
          <span>{t(row.proxy_bound ? "hosted.withProxy" : "hosted.noProxy")}</span>
          <small>{row.credential_state === "revoked" ? t("hosted.revoked") : row.verified_models.join(", ") || t("hosted.unchecked")}</small>
          {poolView && pool?.action && pool.revision && page.pool_access && <div className="hosted-device-actions">
            <Button disabled={busy} onClick={() => {
              if (pool.action && pool.revision) onReview({ source_id: row.id, action: pool.action, expected_revision: pool.revision });
            }}>{t(`hostedPool.${pool.action}`)}</Button>
          </div>}
          {review?.source_id === row.id && pending}
        </li>;
      })}
    </ul>}
  </>;
}
