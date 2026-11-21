import { useEffect, useRef, useState } from "react";
import { Cable } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands, type ApplicationConnectionSetting } from "../../api/commands";
import { IconButton, OptionMenu } from "../../components/Ui";
import { useProxyPool } from "../../hooks/useProxyPool";

const failureKeys = {
  connection_changed: "integrations.connectionChanged",
  connection_not_found: "integrations.connectionMissing",
  connection_unsupported: "integrations.connectionUnsupported",
  connection_auth_unsupported: "integrations.connectionAuthUnsupported",
} as const;
type FailureKey = typeof failureKeys[keyof typeof failureKeys]
  | "integrations.connectionLoadFailed" | "integrations.connectionSaveFailed";

const statusKeys = {
  default: "integrations.connectionDefault",
  saved: "integrations.connectionSaved",
  legacy: "integrations.connectionLegacy",
  changed: "integrations.connectionEdited",
  not_saved: "integrations.connectionSaveFailed",
  unsupported: "integrations.connectionUnsupported",
} as const;

const application = "claude-desktop";

export function ClaudeDesktopConnectionControl() {
  const { t } = useTranslation();
  const { pool, failed: poolFailed, load } = useProxyPool();
  const [setting, setSetting] = useState<ApplicationConnectionSetting>();
  const [failure, setFailure] = useState<FailureKey | null>(null);
  const [saving, setSaving] = useState(false);
  const [reload, setReload] = useState(0);
  const generation = useRef(0);
  const pending = useRef(false);

  useEffect(() => {
    const current = ++generation.current;
    setSetting(undefined);
    setFailure(null);
    setSaving(false);
    pending.current = false;
    void relayCommands.getApplicationConnection(application).then((value) => {
      if (current === generation.current) setSetting(value);
    }).catch(() => {
      if (current === generation.current) setFailure("integrations.connectionLoadFailed");
    });
    return () => { generation.current++; };
  }, [application, reload]);

  useEffect(() => {
    const refresh = () => { if (!pending.current) setReload((version) => version + 1); };
    window.addEventListener("focus", refresh);
    return () => window.removeEventListener("focus", refresh);
  }, []);

  const selected = setting?.proxyId;
  const save = async (value: string) => {
    const proxyId = value || null;
    if (!setting || pending.current || (proxyId === selected && ["default", "saved"].includes(setting.status))) return;
    const current = generation.current;
    pending.current = true;
    setSaving(true);
    setFailure(null);
    try {
      const next = await relayCommands.setApplicationConnection(application, proxyId, setting.revision);
      if (current === generation.current) setSetting(next);
    } catch (error) {
      if (current !== generation.current) return;
      const code = error && typeof error === "object" && "code" in error ? String(error.code) : "";
      setFailure(Object.hasOwn(failureKeys, code)
        ? failureKeys[code as keyof typeof failureKeys] : "integrations.connectionSaveFailed");
    } finally {
      if (current === generation.current) {
        pending.current = false;
        setSaving(false);
      }
    }
  };

  const label = t("integrations.applicationConnection");
  const options = [
    { value: "", label: t("integrations.connectionDefault") },
    ...(pool?.entries ?? []).filter((entry) => setting?.nativeSupported || entry.id === selected)
      .map((entry) => ({ value: entry.id, label: entry.label || entry.endpoint })),
  ];
  const missing = Boolean(selected && pool && !options.some(({ value }) => value === selected));
  if (selected && !options.some(({ value }) => value === selected)) options.push({
    value: selected,
    label: t(pool || poolFailed ? "integrations.connectionMissing" : "common.loading"),
  });

  const entry = pool?.entries.find(({ id }) => id === selected);
  const problem = failure ?? (missing ? "integrations.connectionMissing"
    : selected && poolFailed ? "integrations.connectionLoadFailed" : null);
  const checking = saving || (!problem && (selected === undefined || (selected && !pool)));
  const check = entry?.lastCheck;
  const nativeError = setting && !["default", "saved"].includes(setting.status);
  const status = problem ? "error" : checking ? "pending" : check?.errorCode ? "error"
    : nativeError ? "error" : setting?.status === "saved" ? "unverified" : "default";
  const statusText = problem ? t(problem) : checking ? t("common.loading")
    : nativeError ? t(statusKeys[setting.status]) : check?.errorCode
    ? t(`proxies.checkErrors.${check.errorCode}`, { defaultValue: t("proxies.checkFailed") })
    : t(statusKeys[setting?.status ?? "default"]);
  const retry = () => {
    if (pending.current) return;
    void load();
    setReload((version) => version + 1);
  };
  return <section className="gateway-setting-row application-connection" data-application-connection={application}>
    <header className="codex-feature-heading">
      <IconButton className={`gateway-config-icon application-connection-status is-${status}`}
        label={`${t("integrations.connectionStatus")}: ${statusText}`} title={statusText} busy={Boolean(checking)}
        icon={<Cable aria-hidden />} onClick={retry} disabled={saving} />
      <h2>{label}</h2>
    </header>
    <OptionMenu className="field-option-menu" label={label} value={selected ?? ""} options={options}
      disabled={saving || selected === undefined || (!selected && (!setting?.nativeSupported || !options[1]))}
      onChange={(value) => void save(value)} />
  </section>;
}
