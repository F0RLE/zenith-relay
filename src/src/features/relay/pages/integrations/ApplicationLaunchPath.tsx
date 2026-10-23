import { useEffect, useState } from "react";
import { FolderOpen } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { LaunchApplication, LaunchPathSetting } from "../../api/types";
import { Button, IconButton } from "../../components/Ui";

export function ApplicationLaunchPath({ application }: { application: LaunchApplication }) {
  const { t } = useTranslation();
  const [setting, setSetting] = useState<LaunchPathSetting | null>(null);
  const [draft, setDraft] = useState("");
  const [pending, setPending] = useState<"save" | "pick" | null>(null);
  const [failure, setFailure] = useState<"load" | "save" | "pick" | null>(null);
  const [saved, setSaved] = useState(false);
  const [reload, setReload] = useState(0);
  useEffect(() => {
    let active = true;
    void relayCommands.getApplicationLaunchPath(application).then((value) => {
      if (active) { setSetting(value); setDraft(value.path ?? ""); setFailure(null); }
    }).catch(() => { if (active) setFailure("load"); });
    return () => { active = false; };
  }, [application, reload]);

  const save = async (path: string | null) => {
    setPending("save");
    setFailure(null);
    setSaved(false);
    try {
      const value = await relayCommands.setApplicationLaunchPath(application, path);
      setSetting(value);
      setDraft(value.path ?? "");
      setSaved(true);
    } catch { setFailure("save"); }
    finally { setPending(null); }
  };
  const pick = async () => {
    setPending("pick");
    setFailure(null);
    setSaved(false);
    try {
      const path = await relayCommands.pickApplicationLaunchPath(application, draft.trim() || setting?.detectedPath || null);
      if (path !== null) setDraft(path);
    } catch { setFailure("pick"); }
    finally { setPending(null); }
  };
  const disabled = !setting || Boolean(pending);
  const changed = draft.trim() !== (setting?.path ?? "");
  return <form className="application-launch-path" data-application={application} onSubmit={(event) => { event.preventDefault(); if (!disabled && changed) void save(draft.trim() || null); }}>
    <label className="relay-field">
      <span>{t("integrations.launchPath")}</span>
      <input
        value={draft}
        onChange={(event) => { setDraft(event.target.value); setSaved(false); setFailure(null); }}
        placeholder={setting?.detectedPath?.startsWith("shell:AppsFolder\\")
          ? t("integrations.launchPathWindowsApp")
          : setting?.detectedPath ?? t(setting ? "integrations.launchPathNotDetected" : "common.loading")}
        disabled={disabled}
        autoComplete="off"
        spellCheck={false}
      />
    </label>
    <div className="application-launch-path-actions">
      <IconButton label={t("integrations.chooseApplication")} disabled={disabled} busy={pending === "pick"} icon={<FolderOpen aria-hidden />} onClick={() => void pick()} />
      <Button type="submit" variant="primary" disabled={disabled || !changed} busy={pending === "save"}>{t("common.save")}</Button>
    </div>
    {failure ? <div className="application-launch-path-feedback" role="alert">
      <span>{t(failure === "load" ? "integrations.launchPathLoadFailed" : "integrations.launchPathInvalid")}</span>
      {failure === "load" ? <Button onClick={() => setReload((value) => value + 1)}>{t("common.retry")}</Button> : null}
    </div> : null}
    {saved ? <small role="status">{t("feedback.saved")}</small> : null}
  </form>;
}
