export type RelayStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;

export const RELAY_STORAGE_KEYS = {
  mode: "relay.mode",
  onboarding: "relay.onboarding",
  theme: "relay.theme",
  profileSwitchBackupPrompt: "relay.profileSwitchBackupPrompt",
  codexPoolOauthSelection: "relay.codexPoolOauthSelection",
  legacyCodexPoolOauthSelection: "relay.codexPoolOauthAccountId",
  accountIdentitiesVisible: "relay.accountIdentitiesVisible",
  accountValueVisible: "relay.accountValueVisible",
  legacyPoolEconomicsVisible: "relay.poolEconomicsVisible",
  launchApplicationAfterConnect: "relay.launchApplicationAfterConnect",
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

export function readAccountValueVisibility(storage: RelayStorage | undefined = browserStorage()) {
  const value = readRelayPreference(RELAY_STORAGE_KEYS.accountValueVisible, "", storage)
    || readRelayPreference(RELAY_STORAGE_KEYS.legacyPoolEconomicsVisible, "", storage)
    || "true";
  writeRelayPreference(RELAY_STORAGE_KEYS.accountValueVisible, value, storage);
  removeRelayPreference(RELAY_STORAGE_KEYS.legacyPoolEconomicsVisible, storage);
  return value !== "false";
}

export function writeAccountValueVisibility(
  visible: boolean,
  storage: RelayStorage | undefined = browserStorage(),
) {
  writeRelayPreference(RELAY_STORAGE_KEYS.accountValueVisible, String(visible), storage);
  removeRelayPreference(RELAY_STORAGE_KEYS.legacyPoolEconomicsVisible, storage);
}

export function readLaunchApplicationAfterConnect(
  storage: RelayStorage | undefined = browserStorage(),
) {
  return readRelayPreference(RELAY_STORAGE_KEYS.launchApplicationAfterConnect, "0", storage) === "1";
}

export function writeLaunchApplicationAfterConnect(
  enabled: boolean,
  storage: RelayStorage | undefined = browserStorage(),
) {
  writeRelayPreference(RELAY_STORAGE_KEYS.launchApplicationAfterConnect, enabled ? "1" : "0", storage);
}

function browserStorage(): RelayStorage | undefined {
  try {
    return globalThis.localStorage;
  } catch {
    return undefined;
  }
}
