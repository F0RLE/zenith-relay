import { useCallback, useState, type Dispatch, type SetStateAction } from "react";
import type { PageId, RelayMode } from "../api/types";
import {
  RELAY_STORAGE_KEYS,
  readAccountValueVisibility,
  readCodexPoolOauthSelection,
  readRelayPreference,
  removeRelayPreference,
  writeAccountValueVisibility,
  writeRelayPreference,
} from "./relayPreferences";

type RelayPreferencesInput = {
  setMode: (mode: RelayMode) => void;
  setPage: (page: PageId) => void;
  setRevealedAccountIdentities: Dispatch<SetStateAction<Record<string, string>>>;
};

/** Own persisted UI preferences so the runtime provider only coordinates data. */
export function useRelayPreferences({
  setMode,
  setPage,
  setRevealedAccountIdentities,
}: RelayPreferencesInput) {
  const [onboardingComplete, setOnboardingComplete] = useState(
    () => readRelayPreference(RELAY_STORAGE_KEYS.onboarding, "0") === "1",
  );
  const [theme, setThemeState] = useState<"system" | "light" | "dark">(
    () => readRelayPreference(RELAY_STORAGE_KEYS.theme, "system") as "system" | "light" | "dark",
  );
  const [profileSwitchBackupPrompt, setProfileSwitchBackupPromptState] = useState(
    () => readRelayPreference(RELAY_STORAGE_KEYS.profileSwitchBackupPrompt, "1") !== "0",
  );
  const [codexPoolOauthSelection, setCodexPoolOauthSelectionState] = useState(
    readCodexPoolOauthSelection,
  );
  const [accountIdentitiesVisible, setAccountIdentitiesVisibleState] = useState(
    () => readRelayPreference(RELAY_STORAGE_KEYS.accountIdentitiesVisible, "0") === "1",
  );
  const [accountValueVisible, setAccountValueVisibleState] = useState(readAccountValueVisibility);

  const finishOnboarding = useCallback((nextMode: RelayMode) => {
    writeRelayPreference(RELAY_STORAGE_KEYS.onboarding, "1");
    setOnboardingComplete(true);
    setMode(nextMode);
  }, [setMode]);

  const resetOnboarding = useCallback(() => {
    removeRelayPreference(RELAY_STORAGE_KEYS.onboarding);
    setOnboardingComplete(false);
    setPage("overview");
  }, [setPage]);

  const setTheme = useCallback((next: "system" | "light" | "dark") => {
    writeRelayPreference(RELAY_STORAGE_KEYS.theme, next);
    setThemeState(next);
  }, []);

  const setProfileSwitchBackupPrompt = useCallback((enabled: boolean) => {
    writeRelayPreference(RELAY_STORAGE_KEYS.profileSwitchBackupPrompt, enabled ? "1" : "0");
    setProfileSwitchBackupPromptState(enabled);
  }, []);

  const setCodexPoolOauthSelection = useCallback((selection: string) => {
    writeRelayPreference(RELAY_STORAGE_KEYS.codexPoolOauthSelection, selection);
    removeRelayPreference(RELAY_STORAGE_KEYS.legacyCodexPoolOauthSelection);
    setCodexPoolOauthSelectionState(selection);
  }, []);

  const setAccountIdentitiesVisible = useCallback((visible: boolean) => {
    writeRelayPreference(RELAY_STORAGE_KEYS.accountIdentitiesVisible, visible ? "1" : "0");
    setAccountIdentitiesVisibleState(visible);
    if (!visible) setRevealedAccountIdentities({});
  }, [setRevealedAccountIdentities]);

  const setAccountValueVisible = useCallback((visible: boolean) => {
    writeAccountValueVisibility(visible);
    setAccountValueVisibleState(visible);
  }, []);

  return {
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
  };
}
