import { useEffect, useRef, useState } from "react";
import { Network } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { ApplicationProtocolSetting, LaunchApplication } from "../../api/types";
import { Button, OptionMenu } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

export function ApplicationProtocolControl({ application }: { application: LaunchApplication }) {
  const { t } = useTranslation();
  const { mode, perform } = useRelayState();
  const [setting, setSetting] = useState<ApplicationProtocolSetting | null>(null);
  const [failure, setFailure] = useState<"load" | "save" | null>(null);
  const [saving, setSaving] = useState(false);
  const [reload, setReload] = useState(0);
  const generation = useRef(0);
  const pending = useRef(false);
  useEffect(() => {
    const current = ++generation.current;
    setSetting(null);
    setFailure(null);
    setSaving(false);
    pending.current = false;
    if (mode === "local") {
      void relayCommands.getApplicationUpstreamProtocol(application).then((value) => {
        if (generation.current === current) setSetting(value);
      }).catch(() => { if (generation.current === current) setFailure("load"); });
    }
    return () => { generation.current++; };
  }, [application, mode, reload]);

  if (mode !== "local") return null;
  const policies = [...(setting?.available ?? [])];
  if (setting && !policies.includes(setting.policy)) policies.push(setting.policy);
  const options = policies.map((policy) => ({ value: policy, label: t(`integrations.protocols.${policy}`) }));
  const save = async (value: string) => {
    if (!setting || pending.current || value === setting.policy) return;
    const policy = setting.available.find((candidate) => candidate === value);
    if (!policy) return;
    const current = generation.current;
    pending.current = true;
    setSaving(true);
    setFailure(null);
    try {
      const ok = await perform(`application-protocol-${application}`, async () => {
        const next = await relayCommands.setApplicationUpstreamProtocol(application, policy, setting.policy);
        if (generation.current === current) setSetting(next);
      }, "feedback.saved", { skipRefresh: true, uiLock: false });
      if (!ok && generation.current === current) setFailure("save");
    } finally {
      if (generation.current === current) { pending.current = false; setSaving(false); }
    }
  };
  return <section className="gateway-setting-row application-protocol-control" data-application-protocol={application}>
    <header className="codex-feature-heading">
      <span className="gateway-config-icon"><Network aria-hidden /></span>
      <div><h2>{t("integrations.protocolTitle")}</h2><p>{t("integrations.protocolDescription")}</p></div>
    </header>
    <div className="application-protocol-options">
      <OptionMenu className="field-option-menu" label={t("integrations.protocolTitle")} value={setting?.policy ?? "auto"}
        options={options} disabled={!setting || saving} onChange={(value) => void save(value)} />
      {setting?.reconnectRequired && setting.policy !== "auto" ? <small role="status">{t("integrations.protocolReconnect")}</small> : null}
      {failure ? <div className="application-launch-path-feedback" role="alert">
        <span>{t(failure === "load" ? "integrations.protocolLoadFailed" : "integrations.protocolSaveFailed")}</span>
        <Button disabled={saving} onClick={() => setReload((value) => value + 1)}>{t("common.retry")}</Button>
      </div> : null}
    </div>
  </section>;
}
