import { useEffect, useId, useMemo, useRef, useState } from "react";
import { Check, Globe2, Loader2, Search } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { AccountSummary, AccountTimezoneSource } from "../api/types";
import { useRequestTimezone } from "../hooks/useRequestTimezone";
import { useProxyPool } from "../hooks/useProxyPool";
import { useRelayState } from "../state/RelayStateProvider";
import { DEFAULT_REQUEST_TIMEZONE, timezoneDisplayName, timezoneOffset, timezoneOptions, timezoneSearchText } from "../requestTimezone";
import { Button, Dialog, OptionMenu } from "./Ui";

type AccountTimezoneBinding = "inherit" | "enabled" | "disabled";

export function AccountTimezoneDialog({ account, onClose }: { account: AccountSummary; onClose: () => void }) {
  const { t } = useTranslation();
  const { supported, policy, busy, persist } = useRequestTimezone();
  const { mode, runtime } = useRelayState();
  const localProxyStorage = mode === "local" && !account.recoveryRequired;
  const { pool: proxies, failed: proxyLoadFailed, load: reloadProxies } = useProxyPool(localProxyStorage);
  const bindingName = useId();
  const [expected] = useState(() => structuredClone(policy));
  const [selected, setSelected] = useState(expected.accountTimezones[account.id] ?? "");
  const [source, setSource] = useState<AccountTimezoneSource>(
    expected.accountTimezoneSources[account.id] ?? "account",
  );
  const [binding, setBinding] = useState<AccountTimezoneBinding>(() => {
    const value = expected.accountTimezoneBindings[account.id];
    return value === undefined ? "inherit" : value ? "enabled" : "disabled";
  });
  const [date] = useState(() => new Date());
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const savePending = useRef(false);
  const proxyId = account.proxyMode === "common" ? runtime?.gateway.commonProxyId
    : account.proxyMode === "direct" ? null : account.proxyId;
  const hasProxy = Boolean(proxyId) || account.proxyMode === "account"
    || (account.proxyMode === "common" && Boolean(runtime?.gateway.commonProxyConfigured));
  const proxy = proxies?.entries.find((entry) => entry.id === proxyId);
  const effectiveEnabled = binding === "enabled" || (binding === "inherit" && expected.enabled);
  const effectiveSource = binding === "disabled" ? null : effectiveEnabled ? source : "proxy";
  const pickerVisible = effectiveSource === "account";
  const usesProxy = effectiveSource === "proxy";
  const effectiveZone = pickerVisible ? selected || DEFAULT_REQUEST_TIMEZONE
    : usesProxy ? proxy?.timezone ?? null : null;
  const proxyPending = usesProxy && hasProxy && localProxyStorage && !proxies && !proxyLoadFailed;
  const proxyUnknown = usesProxy && hasProxy
    && (!localProxyStorage || proxyLoadFailed || (proxies !== null && !proxy));
  let previewHint: string | undefined = t("gateway.timezone.noRewriteHint");
  if (proxyUnknown) {
    previewHint = t(localProxyStorage && proxyLoadFailed
      ? "proxies.storageUnavailable" : "gateway.timezone.proxyPreviewUnavailable");
  } else if (usesProxy) {
    previewHint = proxy?.endpoint ?? t(hasProxy ? "gateway.timezone.proxyCurrentHint" : "gateway.timezone.proxyNotAssigned");
  } else if (effectiveZone) {
    previewHint = undefined;
  }
  const save = async () => {
    if (busy || savePending.current) return;
    savePending.current = true;
    setSaving(true);
    setSaveError(null);
    const accountTimezones = { ...expected.accountTimezones };
    if (selected) accountTimezones[account.id] = selected;
    else delete accountTimezones[account.id];
    const accountTimezoneBindings = { ...expected.accountTimezoneBindings };
    if (binding === "inherit") delete accountTimezoneBindings[account.id];
    else accountTimezoneBindings[account.id] = binding === "enabled";
    const accountTimezoneSources = { ...expected.accountTimezoneSources };
    if (source === "proxy") accountTimezoneSources[account.id] = "proxy";
    else delete accountTimezoneSources[account.id];
    try {
      const ok = await persist({ ...expected, accountTimezones, accountTimezoneBindings, accountTimezoneSources }, expected, {
        onError: (error, key) => setSaveError(t(error.code === "conflict" || error.code === "configuration_revision_stale"
          ? "gateway.timezone.staleHint" : key)),
      });
      if (ok) onClose();
    } finally {
      savePending.current = false;
      setSaving(false);
    }
  };
  if (!supported || account.recoveryRequired) return null;
  const modes: Array<{ value: AccountTimezoneBinding; label: string }> = [
    { value: "inherit", label: t("gateway.timezone.useGlobal") },
    { value: "enabled", label: t("gateway.timezone.bindingTitle") },
    { value: "disabled", label: t("gateway.timezone.bindingOff") },
  ];
  return <Dialog
    className="account-timezone-dialog"
    title={t("gateway.timezone.accountTitle")}
    subtitle={<span className="account-timezone-identity">{account.label}</span>}
    onClose={() => { if (!savePending.current) onClose(); }}
    footer={<>
      <Button variant="secondary" disabled={saving} onClick={onClose}>{t("common.cancel")}</Button>
      <Button variant="primary" disabled={Boolean(busy)} busy={saving} onClick={() => void save()}>{t("common.save")}</Button>
    </>}
  >
    <div className="account-timezone-content">
      <fieldset className="account-timezone-modes">
        <legend className="sr-only">{t("gateway.timezone.accountTitle")}</legend>
        {modes.map((item) => <label key={item.value} data-selected={binding === item.value}>
          <input type="radio" name={bindingName} checked={binding === item.value}
            disabled={saving} onChange={() => setBinding(item.value)} />
          <span>{item.label}</span>
        </label>)}
      </fieldset>
      {binding === "inherit" ? <p className="form-note">{t(expected.enabled
        ? "gateway.timezone.bindingInheritedEnabled" : "gateway.timezone.bindingInheritedDisabled")}</p> : null}
      {effectiveEnabled ? <div className="account-timezone-source">
        <span>{t("gateway.timezone.sourceTitle")}</span>
        <OptionMenu label={t("gateway.timezone.sourceTitle")} value={source} align="end"
          options={[
            { value: "account", label: t("gateway.timezone.sourceAccount") },
            { value: "proxy", label: t("gateway.timezone.sourceProxy") },
          ]} disabled={saving} onChange={(value) => setSource(value === "proxy" ? "proxy" : "account")} />
      </div> : null}
      <TimezonePreview zone={effectiveZone} date={date} hint={previewHint} pending={proxyPending} unknown={proxyUnknown} />
      {proxyUnknown && localProxyStorage ? <Button variant="ghost" onClick={() => void reloadProxies()}>{t("common.retry")}</Button> : null}
      {pickerVisible ? <TimezonePicker value={selected || DEFAULT_REQUEST_TIMEZONE} date={date}
        disabled={saving} label={t("gateway.timezone.accountTitle")} onChange={(zone) => {
          setSelected(zone === DEFAULT_REQUEST_TIMEZONE ? "" : zone);
          setSaveError(null);
        }} /> : null}
      {saveError ? <p className="account-timezone-error" role="alert">{saveError}</p> : null}
    </div>
  </Dialog>;
}

function TimezonePreview({ zone, date, hint, pending = false, unknown = false }: {
  zone: string | null; date: Date; hint?: string | undefined; pending?: boolean; unknown?: boolean;
}) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const visibleZone = pending || unknown ? null : zone;
  const displayName = visibleZone ? timezoneDisplayName(visibleZone, date, t, locale) : null;
  const offset = visibleZone ? timezoneOffset(visibleZone, date) : "";
  return <div className="account-timezone-current" role="status">
    <span className="account-timezone-globe">{pending ? <Loader2 className="spin" aria-hidden /> : <Globe2 aria-hidden />}</span>
    <div>
      <span className="account-timezone-eyebrow">{t("gateway.timezone.effective")}</span>
      <strong>{pending ? t("common.loading") : unknown ? t("gateway.timezone.unknown")
        : displayName ?? t("gateway.timezone.noRewrite")}</strong>
      {visibleZone ? <code>{visibleZone}</code> : null}
      {hint && !pending ? <small>{hint}</small> : null}
    </div>
    {offset && offset !== displayName ? <span className="account-timezone-offset">{offset}</span> : null}
  </div>;
}

function TimezonePicker({ value, date, disabled, label, onChange }: {
  value: string; date: Date; disabled: boolean; label: string; onChange: (zone: string) => void;
}) {
  const { t, i18n } = useTranslation();
  const { loadCatalog } = useRequestTimezone();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const name = useId();
  const [zones, setZones] = useState<string[] | null>(null);
  const [failed, setFailed] = useState(false);
  const [retry, setRetry] = useState(0);
  const [query, setQuery] = useState("");
  const list = useRef<HTMLDivElement>(null);
  const positioned = useRef(false);
  useEffect(() => {
    let disposed = false;
    setFailed(false);
    void loadCatalog().then((catalog) => { if (!disposed) setZones(catalog); })
      .catch(() => { if (!disposed) setFailed(true); });
    return () => { disposed = true; };
  }, [loadCatalog, retry]);
  const options = useMemo(() => timezoneOptions(zones ?? [], date, t, locale, value), [zones, date, t, locale, value]);
  const terms = timezoneSearchText(query.trim()).split(/\s+/);
  const visible = options.filter((option) => terms.every((term) => option.search.includes(term)));
  useEffect(() => {
    if (!zones || positioned.current || query) return;
    const selected = list.current?.querySelector<HTMLElement>('[data-selected="true"]');
    if (selected && list.current) {
      list.current.scrollTop += selected.getBoundingClientRect().top - list.current.getBoundingClientRect().top - 16;
    }
    positioned.current = true;
  }, [zones, query]);
  return <div className="account-timezone-picker">
    <label className="search-field account-timezone-search">
      <Search aria-hidden />
      <input type="search" aria-label={t("gateway.timezone.search")} placeholder={t("gateway.timezone.search")}
        value={query} onChange={(event) => setQuery(event.target.value)} />
    </label>
    {failed ? <div className="account-timezone-empty" role="alert">
      <p>{t("gateway.timezone.catalogUnavailable")}</p>
      <Button variant="secondary" onClick={() => setRetry((count) => count + 1)}>{t("common.retry")}</Button>
    </div> : zones == null ? <div className="account-timezone-empty" role="status">
      <Loader2 className="spin" aria-hidden /><span>{t("common.loading")}</span>
    </div> : <div className="account-timezone-list" ref={list} role="radiogroup" aria-label={label}>
      {visible.map((option) => <label key={option.zone} className="account-timezone-option" data-selected={value === option.zone}>
        <input className="sr-only" type="radio" name={name} value={option.zone} checked={value === option.zone}
          disabled={disabled} onChange={() => onChange(option.zone)} />
        <span className="account-timezone-check"><Check aria-hidden /></span>
        <span className="account-timezone-option-name"><strong>{option.city}</strong><code>{option.zone}</code></span>
        {option.offset && option.offset !== option.city ? <span className="account-timezone-offset">{option.offset}</span> : null}
      </label>)}
      {!visible.length ? <div className="account-timezone-empty"><p>{t("gateway.timezone.noResults")}</p></div> : null}
    </div>}
  </div>;
}
