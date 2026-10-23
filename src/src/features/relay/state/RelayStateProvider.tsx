import { useCallback, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { setWindowBackgroundColor } from "../../../platform/desktop";
import { relayCommands } from "../api/commands";
import type { ProfileActivation } from "../api/types";
import { useConfirm } from "../components/Ui";
import { buildAccountIdentityIndex, displayAccountIdentity } from "./accountIdentity";
import { useAccountIdentityReveal } from "./useAccountIdentityReveal";
import { useRelayOperations } from "./useRelayOperations";
import { useRelayPreferences } from "./useRelayPreferences";
import { useRelayRuntime } from "./useRelayRuntime";
import { useRelayUsage } from "./useRelayUsage";
import { RelayStateContexts, type PerformOptions, type RelayContextValue, type RelayUsageContextValue } from "./relayStateContext";
import { createRuntimeAccountLabelProjector } from "./runtimeDisplay";

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
    loadLocalUsage,
    localUsagePage,
    resetUsage,
  } = useRelayUsage(relayCommands);
  const {
    mode,
    setMode,
    page,
    setPage,
    runtime,
    runtimeActivity,
    runtimeRevision,
    usageRevision,
    loading,
    refresh,
  } = useRelayRuntime({
    cancelOperations,
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
    launchApplicationAfterConnect,
    setLaunchApplicationAfterConnect,
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
  const canRevealAccountIdentities = mode === "local";
  const accountIdentitiesBusy = useAccountIdentityReveal({
    accounts: runtime?.accounts ?? [],
    canReveal: canRevealAccountIdentities,
    identitiesVisible: accountIdentitiesVisible,
    mode,
    setRevealedIdentities: setRevealedAccountIdentities,
  });
  const accountIndex = useMemo(() => buildAccountIdentityIndex(runtime?.accounts ?? []), [runtime?.accounts]);

  const perform = useCallback(async (
    operationId: string,
    work: () => Promise<unknown>,
    successKey?: string,
    options?: PerformOptions,
  ) => performOperation(operationId, work, refresh, successKey, options), [performOperation, refresh]);

  const activateCodexProfile = useCallback(async (
    profileOperationId: string,
    work: () => Promise<ProfileActivation>,
    launchAfter = false,
  ) => {
    if (profileSwitchBackupPrompt && !await confirm(t("profiles.switchBackupMessage"), {
      title: t("profiles.switchBackupTitle"),
      confirmLabel: t("profiles.switchBackupAction"),
    })) return false;
    return perform(profileOperationId, work, launchAfter ? "feedback.launched" : "feedback.profileAttached",
      { backgroundRefresh: true, ...(launchAfter ? { afterWork: relayCommands.launchManagedCodex } : {}) });
  }, [confirm, perform, profileSwitchBackupPrompt, t]);

  const setCodexBackgroundTasksEnabled = useCallback((enabled: boolean) => perform(
    "codex-background-tasks",
    mode === "local"
      ? () => relayCommands.setCodexBackgroundTasks(enabled)
      : () => Promise.reject(new Error(t("errors.chatgpt_background_tasks_unavailable"))),
    "feedback.saved",
    { backgroundRefresh: true, uiLock: false },
  ), [mode, perform, t]);

  const restartManagedCodexIfRunning = useCallback(async () => {
    const wasRunning = await relayCommands.stopManagedCodex();
    if (wasRunning) await relayCommands.launchManagedCodex();
  }, []);

  const setCodexWebsocketsEnabled = useCallback(async (enabled: boolean) => {
    const saved = await perform("codex-websockets", async () => {
      if (mode === "local") {
        return relayCommands.setCodexWebsockets(enabled);
      }
      throw new Error(t("errors.chatgpt_websockets_unavailable"));
    }, undefined, { backgroundRefresh: true, uiLock: false });
    if (!saved) return false;
    // Restart failure must not undo a setting already saved in the local runtime and profile.
    await perform("codex-websockets-restart", restartManagedCodexIfRunning,
      "feedback.saved", { skipRefresh: true, uiLock: false });
    return true;
  }, [mode, perform, restartManagedCodexIfRunning, t]);

  const setRouteRecoveryEnabled = useCallback((enabled: boolean) => perform(
    "route-recovery",
    mode === "local"
      ? () => relayCommands.setChatgptRetryUntilAvailable(enabled)
      : () => Promise.reject(new Error(t("errors.chatgpt_retry_until_available_unavailable"))),
    "feedback.saved",
    { backgroundRefresh: true, uiLock: false },
  ), [mode, perform, t]);

  const setBlockDegradedRoutesEnabled = useCallback((enabled: boolean) => perform(
    "block-degraded-routes",
    mode === "local"
      ? () => relayCommands.setBlockDegradedRoutes(enabled)
      : () => Promise.reject(new Error(t("errors.block_degraded_routes_unavailable"))),
    "feedback.saved",
    { backgroundRefresh: true, uiLock: false },
  ), [mode, perform, t]);

  useEffect(() => {
    const systemTheme = window.matchMedia("(prefers-color-scheme: dark)");
    const applyTheme = () => {
      document.documentElement.dataset["theme"] = theme;
      const dark = theme === "dark" || (theme === "system" && systemTheme.matches);
      void setWindowBackgroundColor(dark ? "#141414" : "#f4f3ee");
    };
    applyTheme();
    if (theme !== "system") return;
    systemTheme.addEventListener("change", applyTheme);
    return () => systemTheme.removeEventListener("change", applyTheme);
  }, [theme]);

  const accountDisplayName = useCallback((accountId?: string | null, fallbackLabel?: string | null) => {
    return displayAccountIdentity({
      index: accountIndex,
      ...(accountId !== undefined ? { accountId } : {}),
      ...(fallbackLabel !== undefined ? { fallbackLabel } : {}),
      identitiesVisible: accountIdentitiesVisible,
      canReveal: canRevealAccountIdentities,
      mode,
      revealedIdentities: revealedAccountIdentities,
    });
  }, [accountIdentitiesVisible, accountIndex, canRevealAccountIdentities, mode, revealedAccountIdentities]);

  const projectAccountLabels = useMemo(createRuntimeAccountLabelProjector, []);
  const displayRuntime = useMemo(
    () => projectAccountLabels(runtime, accountDisplayName),
    [accountDisplayName, projectAccountLabels, runtime],
  );
  const relayContextValue = useMemo<RelayContextValue>(() => ({
    mode,
    setMode,
    page,
    setPage,
    runtime: displayRuntime,
    runtimeRevision,
    accountIdentitiesVisible,
    accountIdentitiesBusy,
    canRevealAccountIdentities,
    setAccountIdentitiesVisible,
    accountValueVisible,
    setAccountValueVisible,
    accountDisplayName,
    loading,
    busy,
    feedback,
    refresh,
    perform,
    activateCodexProfile,
    clearFeedback,
    onboardingComplete,
    finishOnboarding,
    resetOnboarding,
    theme,
    setTheme,
    profileSwitchBackupPrompt,
    setProfileSwitchBackupPrompt,
    launchApplicationAfterConnect,
    setLaunchApplicationAfterConnect,
    codexPoolOauthSelection,
    setCodexPoolOauthSelection,
    codexBackgroundTasksEnabled: displayRuntime?.gateway.codexBackgroundTasksEnabled ?? true,
    setCodexBackgroundTasksEnabled,
    setCodexWebsocketsEnabled,
    codexWebsocketsEnabled: displayRuntime?.gateway.codexWebsocketsEnabled ?? true,
    routeRecoveryEnabled: displayRuntime?.gateway.chatgptRetryUntilAvailable ?? false,
    setRouteRecoveryEnabled,
    blockDegradedRoutesEnabled: displayRuntime?.gateway.blockDegradedRoutesEnabled ?? true,
    setBlockDegradedRoutesEnabled,
  }), [
    mode,
    setMode,
    page,
    setPage,
    displayRuntime,
    runtimeRevision,
    accountIdentitiesVisible,
    accountIdentitiesBusy,
    canRevealAccountIdentities,
    setAccountIdentitiesVisible,
    accountValueVisible,
    setAccountValueVisible,
    accountDisplayName,
    loading,
    busy,
    feedback,
    refresh,
    perform,
    activateCodexProfile,
    clearFeedback,
    onboardingComplete,
    finishOnboarding,
    resetOnboarding,
    theme,
    setTheme,
    profileSwitchBackupPrompt,
    setProfileSwitchBackupPrompt,
    launchApplicationAfterConnect,
    setLaunchApplicationAfterConnect,
    codexPoolOauthSelection,
    setCodexPoolOauthSelection,
    setCodexBackgroundTasksEnabled,
    setCodexWebsocketsEnabled,
    setRouteRecoveryEnabled,
    setBlockDegradedRoutesEnabled,
  ]);

  const usage = useMemo<RelayUsageContextValue>(() => ({
    localUsagePage,
    loadLocalUsage,
    usageRevision,
  }), [loadLocalUsage, localUsagePage, usageRevision]);

  useEffect(() => {
    document.documentElement.lang = i18n.language.startsWith("ru") ? "ru" : "en";
  }, [i18n.language]);

  return <RelayStateContexts contextValue={relayContextValue} activity={runtimeActivity} usage={usage}>{children}</RelayStateContexts>;
}
