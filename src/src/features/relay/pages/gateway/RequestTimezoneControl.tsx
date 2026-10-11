import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { RequestTimezonePolicy } from "../../api/types";
import { Button, Dialog, ToggleSwitch } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { usePendingFlag } from "../../state/usePendingFlag";

const DEFAULT_POLICY: RequestTimezonePolicy = { enabled: false, accountTimezones: {} };

export function RequestTimezoneControl() {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const controlId = useId();
  const policy = runtime?.gateway.requestTimezone ?? DEFAULT_POLICY;
  const { checked, select } = usePendingFlag(policy.enabled);
  const [draft, setDraft] = useState<RequestTimezonePolicy | null>(null);
  const [expected, setExpected] = useState(DEFAULT_POLICY);
  const [zones, setZones] = useState<string[]>([]);
  if (mode === "zenith" || !runtime || (mode === "remote" && !runtime.capabilities.features.includes("request_timezone_v1"))) return null;
  const accounts = runtime.accounts.filter((account) => !account.remoteLocation);
  const persist = (next: RequestTimezonePolicy, previous = policy) => perform("request-timezone", () => {
    const input = { policy: next, expectedPolicy: previous };
    return mode === "local"
      ? relayCommands.setRequestTimezone(input)
      : relayCommands.remoteAction({ type: "set_request_timezone" }, input);
  }, "feedback.saved", { backgroundRefresh: true });
  const open = () => perform("request-timezone-catalog", async () => {
    const catalog = mode === "local"
      ? await relayCommands.requestTimezoneCatalog()
      : await relayCommands.remoteAction({ type: "request_timezone_catalog" });
    if (!Array.isArray(catalog) || !catalog.every((zone): zone is string => typeof zone === "string")) {
      throw new Error(t("gateway.timezone.catalogUnavailable"));
    }
    setZones(catalog);
    setExpected(structuredClone(policy));
    setDraft(structuredClone(policy));
  });
  const valid = draft != null && Object.values(draft.accountTimezones).every((zone) => zones.includes(zone));
  return <div className="gateway-api-toggle-setting">
    <div className="relay-toggle-setting">
      <label htmlFor={controlId}>
        <strong>{t("gateway.timezone.title")}</strong>
        <span id={`${controlId}-description`}>{t("gateway.timezone.description")}</span>
      </label>
      <ToggleSwitch id={controlId} label={t("gateway.timezone.title")}
        aria-describedby={`${controlId}-description`} checked={checked} disabled={Boolean(busy)}
        onChange={(enabled) => select(enabled, () => persist({ ...policy, enabled }))} />
    </div>
    <Button variant="secondary" disabled={Boolean(busy)} onClick={() => void open()}>{t("gateway.timezone.configure")}</Button>
    {draft ? <Dialog title={t("gateway.timezone.configure")} onClose={() => setDraft(null)} footer={<>
      <Button variant="secondary" onClick={() => setDraft(null)}>{t("common.cancel")}</Button>
      <Button variant="primary" disabled={!valid || Boolean(busy)} busy={busy === "request-timezone"}
        onClick={() => void persist(draft, expected).then((ok) => { if (ok) setDraft(null); })}>{t("common.save")}</Button>
    </>}>
      <div className="relay-form request-timezone-form">
        <p>{t("gateway.timezone.defaultHint")}</p>
        <datalist id={`${controlId}-zones`}>{zones.map((zone) => <option key={zone} value={zone} />)}</datalist>
        {accounts.map((account) => <label key={account.id}>
          <span>{account.label}</span>
          <input aria-label={t("gateway.timezone.accountLabel", { name: account.label })} list={`${controlId}-zones`}
            placeholder="America/New_York" value={draft.accountTimezones[account.id] ?? ""}
            onChange={(event) => {
              const accountTimezones = { ...draft.accountTimezones };
              if (event.target.value) accountTimezones[account.id] = event.target.value;
              else delete accountTimezones[account.id];
              setDraft({ ...draft, accountTimezones });
            }} />
        </label>)}
        {!valid ? <p role="alert">{t("gateway.timezone.invalid")}</p> : null}
      </div>
    </Dialog> : null}
  </div>;
}
