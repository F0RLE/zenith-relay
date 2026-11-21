import { useTranslation } from "react-i18next";
import { ChevronDown } from "lucide-react";
import type { HostedHours, HostedOverview, HostedUsage, HostedUsageFilter } from "../../api/types/hostedDevice";
import { Button, CopyButton, OptionMenu } from "../../components/Ui";

const periods = { 24: "usage.daily", 168: "usage.weekly", 720: "usage.monthly" } as const;
const periodOptions = [24, 168, 720] as const;
const states = ["reserved", "dispatched", "completed", "incomplete", "failed", "uncertain"] as const;

export function useHostedFormatting() {
  const { i18n } = useTranslation();
  const locale = i18n.resolvedLanguage;
  return {
    date: (ms: number) => new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(ms),
    count: (n: string) => new Intl.NumberFormat(locale).format(BigInt(n)),
    rate: (n: number) => new Intl.NumberFormat(locale, { maximumFractionDigits: 2 }).format(n),
  };
}

function Period({ hours, onChange, busy }: { hours: HostedHours; onChange: (n: HostedHours) => void; busy: boolean }) {
  const { t } = useTranslation();
  return <OptionMenu label={t("usage.range")} value={String(hours)} disabled={busy} align="start" fitContent
    options={periodOptions.map(n => ({ value: String(n), label: t(periods[n]) }))}
    onChange={value => { const picked = periodOptions.find(n => String(n) === value); if (picked) onChange(picked); }} />;
}

export function HostedOverviewPanel({ value, hours, busy, onChange }: {
  value: HostedOverview | undefined; hours: HostedHours; busy: boolean; onChange: (hours: HostedHours) => void;
}) {
  const { t } = useTranslation();
  const { date, count, rate } = useHostedFormatting();
  const { pool, generation } = value?.metrics ?? {};
  return <>
    <div className="hosted-observation-bar">
      <Period hours={hours} busy={busy} onChange={onChange} />
      {value && <small>{t("hosted.observedAt", { date: date(value.as_of_ms) })}</small>}
    </div>
    {value && <dl className="hosted-metrics">
      <div><dt>{t("usage.requests")}</dt><dd>{count(value.requests)}</dd></div>
      <div><dt>{t("hosted.requestErrors")}</dt><dd>{count(value.errors)}</dd></div>
      <div><dt>{t("usage.tokens")}</dt><dd>{value.total_tokens === null ? "—" : count(value.total_tokens)}</dd>
        {value.requests !== "0" && (value.token_samples !== value.requests || value.partial_tokens !== "0") &&
          <dd className="hosted-metric-note">{t("hosted.tokenCoverage", { samples: count(value.token_samples), requests: count(value.requests), partial: count(value.partial_tokens) })}</dd>}
      </div>
      <div><dt>{t("hosted.activeRequests")}</dt><dd>{count(value.active_requests)}</dd></div>
      <div><dt>{t("hosted.activeAttempts")}</dt><dd>{count(value.active_attempts)}</dd></div>
      {value.metrics && <>
        <div><dt>{t("hosted.poolReady")}</dt><dd>{pool ? `${count(pool.ready)} / ${count(pool.members)}` : "—"}</dd>
          {pool && pool.busy !== "0" && <dd className="hosted-metric-note">{t("hosted.poolBusy", { busy: count(pool.busy) })}</dd>}
        </div>
        <div><dt>{t("usage.generationSpeed")}</dt>
          <dd>{generation ? <>{rate(generation.tokens_per_second)} <small>{t("usage.tokensPerSecondUnit")}</small></> : "—"}</dd>
          {generation && <dd className="hosted-metric-note">{t("hosted.speedCoverage", { samples: count(generation.samples) })}</dd>}
        </div>
      </>}
    </dl>}
  </>;
}

type UsageSelection = Pick<HostedUsageFilter, "hours" | "model" | "execution">;
export function HostedUsagePanel({ value, filter, model, setModel, busy, onChange, canGoBack, onPage }: {
  value: HostedUsage | undefined;
  filter: UsageSelection; model: string; setModel: (v: string) => void; busy: boolean;
  onChange: (filter: UsageSelection) => void; canGoBack: boolean; onPage: (next: boolean) => void;
}) {
  const { t } = useTranslation();
  const { date, count } = useHostedFormatting();
  const modelId = model.trim();
  const validModel = !modelId || /^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$/.test(modelId);
  const search = (changes: Partial<UsageSelection>) => onChange({ ...filter, ...changes });
  return <>
    <div className="hosted-observation-bar">
      <Period hours={filter.hours} busy={busy} onChange={hours => search({ hours })} />
      <OptionMenu label={t("common.status")} value={filter.execution ?? ""} disabled={busy} align="start" fitContent
        options={[{ value: "", label: t("usage.anyStatus") }, ...states.map(value => ({ value, label: t(`hosted.execution.${value}`) }))]}
        onChange={value => search({ execution: states.find(s => s === value) ?? null })} />
    </div>
    <form className="hosted-model-filter" onSubmit={e => {
      e.preventDefault();
      if (validModel && !busy) search({ model: modelId || null });
    }}>
      <label className="relay-field"><span>{t("common.model")}</span>
        <input value={model} onChange={e => setModel(e.target.value)} maxLength={128} autoComplete="off" spellCheck={false}
          placeholder={t("usage.anyModel")} aria-invalid={!validModel} disabled={busy} />
      </label>
      <Button type="submit" disabled={!validModel || busy}>{t("common.search")}</Button>
    </form>
    {value && <>
      {value.items.length === 0 ? <p>{t("hosted.usageEmpty")}</p> : <ul className="hosted-requests">
        {value.items.map(row => <li key={row.request_id}>
          <details>
            <summary>
              <strong>{row.model}</strong>
              <time dateTime={new Date(row.started_at_ms).toISOString()}>{date(row.started_at_ms)}</time>
              <span>{t(`hosted.execution.${row.execution}`)}</span>
              <ChevronDown aria-hidden className="hosted-request-chevron" />
            </summary>
            <dl>
              <div><dt>{t("hosted.requestId")}</dt><dd><code>{row.request_id}</code><CopyButton value={row.request_id} label={t("common.copy")} /></dd></div>
              <div><dt>{t("usage.tokens")}</dt><dd>{row.tokens === null ? "—" : count(row.tokens)}
                {row.partial_tokens && <small>{t("hosted.partialTokens")}</small>}</dd></div>
              {row.diagnostic_code && <div><dt>{t("hosted.diagnostic")}</dt><dd><code>{row.diagnostic_code}</code></dd></div>}
            </dl>
          </details>
        </li>)}
      </ul>}
      {(canGoBack || value.next_before) && <div className="hosted-device-actions">
        {canGoBack && <Button disabled={busy} onClick={() => onPage(false)}>{t("common.back")}</Button>}
        {value.next_before && <Button disabled={busy} onClick={() => onPage(true)}>{t("hosted.next")}</Button>}
      </div>}
    </>}
  </>;
}
