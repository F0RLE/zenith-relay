export type RelayStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;
export type AccountQuotaCalculationMode = "provider" | "zenith_experimental";

export const RELAY_STORAGE_KEYS = {
  mode: "relay.mode",
  onboarding: "relay.onboarding",
  theme: "relay.theme",
  profileSwitchBackupPrompt: "relay.profileSwitchBackupPrompt",
  profileSnapshotBackupBeforeRestore: "relay.profileSnapshotBackupBeforeRestore",
  codexPoolOauthSelection: "relay.codexPoolOauthSelection",
  legacyCodexPoolOauthSelection: "relay.codexPoolOauthAccountId",
  accountIdentitiesVisible: "relay.accountIdentitiesVisible",
  poolEconomicsVisible: "relay.poolEconomicsVisible",
  accountQuotaCalculationMode: "relay.accountQuotaCalculationMode",
} as const;

export function readRelayPreference(
  key: string,
  fallback: string,
  storage: RelayStorage | undefined = browserStorage(),
) {
  try {
    return storage?.getItem(key) ?? fallback;
  } catch {
    return fallback;
  }
}

export function writeRelayPreference(
  key: string,
  value: string,
  storage: RelayStorage | undefined = browserStorage(),
) {
  try {
    storage?.setItem(key, value);
  } catch {
    // Preferences are optional; a restricted browser storage must not block the app.
  }
}

export function removeRelayPreference(
  key: string,
  storage: RelayStorage | undefined = browserStorage(),
) {
  try {
    storage?.removeItem(key);
  } catch {
    // Preferences are optional; a restricted browser storage must not block the app.
  }
}

export function readCodexPoolOauthSelection(storage: RelayStorage | undefined = browserStorage()) {
  const selection = readRelayPreference(RELAY_STORAGE_KEYS.codexPoolOauthSelection, "", storage)
    || readRelayPreference(RELAY_STORAGE_KEYS.legacyCodexPoolOauthSelection, "", storage)
    || "auto";
  writeRelayPreference(RELAY_STORAGE_KEYS.codexPoolOauthSelection, selection, storage);
  removeRelayPreference(RELAY_STORAGE_KEYS.legacyCodexPoolOauthSelection, storage);
  return selection;
}

export function readAccountQuotaCalculationMode(storage: RelayStorage | undefined = browserStorage()): AccountQuotaCalculationMode {
  return readRelayPreference(RELAY_STORAGE_KEYS.accountQuotaCalculationMode, "provider", storage) === "zenith_experimental"
    ? "zenith_experimental"
    : "provider";
}

function browserStorage(): RelayStorage | undefined {
  try {
    return globalThis.localStorage;
  } catch {
    return undefined;
  }
}
