import { ReactNode, useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { setWindowBackgroundColor } from "../../../platform/desktop";
import { relayCommands } from "../api/commands";
import type { ProfileActivation, ProfileBinding } from "../api/types";
import { useConfirm } from "../components/Ui";
import { buildAccountIdentityIndex, displayAccountIdentity } from "./accountIdentity";
import { useAccountIdentityReveal } from "./useAccountIdentityReveal";
import { useRelayOperations } from "./useRelayOperations";
import { useRelayPreferences } from "./useRelayPreferences";
import { useRelayRuntime } from "./useRelayRuntime";
import { useRelayUsage } from "./useRelayUsage";
import { RelayContext, type PerformOptions, type RelayContextValue } from "./relayStateContext";
import { projectRuntimeAccountLabels } from "./runtimeDisplay";

export { useRelayState } from "./relayStateContext";
export type { Feedback } from "./relayStateContext";

export function RelayStateProvider({ children }: { children: ReactNode }) {
  const { i18n, t } = useTranslation();
  const confirm = useConfirm();
  const [revealedAccountIdentities, setRevealedAccountIdentities] = useState<Record<string, string>>({});
  const {
    busy,
    feedback,
    performOperation,
    cancelOperations,
    clearFeedback,
    reportErrorFeedback,
  } = useRelayOperations();
  const {
    localUsagePage,
    remoteUsage,
    remoteUsagePage,
    loadLocalUsage,
    loadRemoteUsage,
    resetUsage,
    clearInactiveUsage,
  } = useRelayUsage(relayCommands);
  const {
    mode,
    setMode,
    page,
    setPage,
    runtime,
    runtimeRevision,
    usageRevision,
    readyState,
    loading,
    refresh,
  } = useRelayRuntime({
    cancelOperations,
    clearInactiveUsage,
    resetUsage,
    reportErrorFeedback,
  });
  const {
    onboardingComplete,
    finishOnboarding,
    resetOnboarding,
    theme,
    setTheme,
    profileSwitchBackupPrompt,
    setProfileSwitchBackupPrompt,
    codexPoolOauthSelection,
    setCodexPoolOauthSelection,
    accountIdentitiesVisible,
    setAccountIdentitiesVisible,
    accountValueVisible,
    setAccountValueVisible,
  } = useRelayPreferences({
    setMode,
    setPage,
    setRevealedAccountIdentities,
  });
  const canRevealAccountIdentities = mode === "local" || (mode === "remote" && Boolean(runtime?.capabilities.features.includes("account_identity_reveal")));
  const accountIdentitiesBusy = useAccountIdentityReveal({
    accounts: runtime?.accounts ?? [],
    canReveal: canRevealAccountIdentities,
    identitiesVisible: accountIdentitiesVisible,
    mode,
    setRevealedIdentities: setRevealedAccountIdentities,
  });
  const accountIndex = useMemo(() => buildAccountIdentityIndex(runtime?.accounts ?? []), [runtime?.accounts]);
  const codexWebsocketsEnabled = runtime?.gateway.codexWebsocketsEnabled ?? true;

  const perform = useCallback(async (
    id: string,
    work: () => Promise<unknown>,
    successKey?: string,
    options?: PerformOptions,
  ) => performOperation(id, work, refresh, successKey, options), [performOperation, refresh]);

  const launchAttachedCodex = useCallback(() => perform(
    "profile-launch",
    relayCommands.launchManagedCodex,
    "feedback.launched",
  ), [perform]);

  const activateCodexProfile = useCallback(async (
    id: string,
    work: () => Promise<ProfileActivation>,
    launchAfter = false,
  ) => {
    if (profileSwitchBackupPrompt && !await confirm(t("profiles.switchBackupMessage"), {
      title: t("profiles.switchBackupTitle"),
      confirmLabel: t("profiles.switchBackupAction"),
    })) return false;
    const activated = await perform(id, work, launchAfter ? undefined : "feedback.profileAttached");
    return activated && (!launchAfter || await launchAttachedCodex());
  }, [confirm, launchAttachedCodex, perform, profileSwitchBackupPrompt, t]);

  const launchCodexProfile = useCallback(async (_binding: ProfileBinding) => {
    const stopped = await perform("profile-stop", relayCommands.stopManagedCodex);
    return stopped && launchAttachedCodex();
  }, [launchAttachedCodex, perform]);

  const setCodexBackgroundTasksEnabled = useCallback((enabled: boolean) => perform(
    "codex-background-tasks",
    mode === "local"
      ? () => relayCommands.setCodexBackgroundTasks(enabled)
      : mode === "remote"
        ? () => relayCommands.setRemoteCodexBackgroundTasks(enabled)
        : () => Promise.reject(new Error("Codex background tasks are not available in hosted mode")),
    "feedback.saved",
  ), [mode, perform]);

  const setCodexWebsocketsEnabled = useCallback((enabled: boolean) => perform(
    "codex-websockets",
    mode === "remote"
      ? async () => {
        const previous = codexWebsocketsEnabled;
        await relayCommands.setRemoteCodexWebsockets(enabled);
        try {
          return await relayCommands.setCodexProfileWebsockets(enabled);
        } catch (error) {
          try {
            await relayCommands.setRemoteCodexWebsockets(previous);
          } catch {
            // Keep the original profile error; the remote action is best-effort rollback.
          }
          throw error;
        }
      }
      : mode === "local"
        ? () => relayCommands.setCodexWebsockets(enabled)
        : () => Promise.reject(new Error("Codex WebSockets are not available in hosted mode")),
    "feedback.saved",
  ), [codexWebsocketsEnabled, mode, perform]);

  useEffect(() => {
    const systemTheme = window.matchMedia("(prefers-color-scheme: dark)");
    const applyTheme = () => {
      document.documentElement.dataset.theme = theme;
      const dark = theme === "dark" || (theme === "system" && systemTheme.matches);
      void setWindowBackgroundColor(dark ? "#121719" : "#f2f5f6");
    };
    applyTheme();
    if (theme !== "system") return;
    systemTheme.addEventListener("change", applyTheme);
    return () => systemTheme.removeEventListener("change", applyTheme);
  }, [theme]);

  const accountDisplayName = useCallback((accountId?: string | null, fallbackLabel?: string | null) => {
    return displayAccountIdentity({
      index: accountIndex,
      accountId,
      fallbackLabel,
      identitiesVisible: accountIdentitiesVisible,
      canReveal: canRevealAccountIdentities,
      mode,
      revealedIdentities: revealedAccountIdentities,
    });
  }, [accountIdentitiesVisible, accountIndex, canRevealAccountIdentities, mode, revealedAccountIdentities]);

  const displayRuntime = useMemo(
    () => projectRuntimeAccountLabels(runtime, accountDisplayName),
    [accountDisplayName, runtime],
  );
  const value = useMemo<RelayContextValue>(() => ({
    mode,
    setMode,
    page,
    setPage,
    runtime: displayRuntime,
    runtimeRevision,
    usageRevision,
    accountIdentitiesVisible,
    accountIdentitiesBusy,
    canRevealAccountIdentities,
    setAccountIdentitiesVisible,
    accountValueVisible,
    setAccountValueVisible,
    accountDisplayName,
    localUsagePage,
    loadLocalUsage,
    remoteUsage,
    remoteUsagePage,
    loadRemoteUsage,
    readyState,
    loading,
    busy,
    feedback,
    refresh,
    perform,
    activateCodexProfile,
    launchCodexProfile,
    clearFeedback,
    onboardingComplete,
    finishOnboarding,
    resetOnboarding,
    theme,
    setTheme,
    profileSwitchBackupPrompt,
    setProfileSwitchBackupPrompt,
    codexPoolOauthSelection,
    setCodexPoolOauthSelection,
    codexBackgroundTasksEnabled: displayRuntime?.gateway.codexBackgroundTasksEnabled ?? true,
    setCodexBackgroundTasksEnabled,
    setCodexWebsocketsEnabled,
    codexWebsocketsEnabled: displayRuntime?.gateway.codexWebsocketsEnabled ?? true,
  }), [
    mode,
    setMode,
    page,
    setPage,
    displayRuntime,
    runtimeRevision,
    usageRevision,
    accountIdentitiesVisible,
    accountIdentitiesBusy,
    canRevealAccountIdentities,
    setAccountIdentitiesVisible,
    accountValueVisible,
    setAccountValueVisible,
    accountDisplayName,
    localUsagePage,
    loadLocalUsage,
    remoteUsage,
    remoteUsagePage,
    loadRemoteUsage,
    readyState,
    loading,
    busy,
    feedback,
    refresh,
    perform,
    activateCodexProfile,
    launchCodexProfile,
    clearFeedback,
    onboardingComplete,
    finishOnboarding,
    resetOnboarding,
    theme,
    setTheme,
    profileSwitchBackupPrompt,
    setProfileSwitchBackupPrompt,
    codexPoolOauthSelection,
    setCodexPoolOauthSelection,
    setCodexBackgroundTasksEnabled,
    setCodexWebsocketsEnabled,
  ]);

  useEffect(() => {
    document.documentElement.lang = i18n.language.startsWith("ru") ? "ru" : "en";
  }, [i18n.language]);

  return <RelayContext.Provider value={value}>{children}</RelayContext.Provider>;
}
