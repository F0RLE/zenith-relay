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
  pendingAccountImportConfirmation: "relay.pendingAccountImportConfirmation",
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
  preferenceValue: string,
  storage: RelayStorage | undefined = browserStorage(),
): boolean {
  try {
    if (!storage) return false;
    storage.setItem(key, preferenceValue);
    return true;
  } catch {
    // Preferences are optional; a restricted browser storage must not block the app.
    return false;
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
  const selection = readMigratedRelayPreference(
    RELAY_STORAGE_KEYS.codexPoolOauthSelection,
    RELAY_STORAGE_KEYS.legacyCodexPoolOauthSelection,
    "none",
    storage,
  );
  if (selection === "auto") {
    writeMigratedRelayPreference(
      RELAY_STORAGE_KEYS.codexPoolOauthSelection,
      RELAY_STORAGE_KEYS.legacyCodexPoolOauthSelection,
      "none",
      storage,
    );
    return "none";
  }
  return selection;
}

export function writeCodexPoolOauthSelection(
  selection: string,
  storage: RelayStorage | undefined = browserStorage(),
) {
  writeMigratedRelayPreference(
    RELAY_STORAGE_KEYS.codexPoolOauthSelection,
    RELAY_STORAGE_KEYS.legacyCodexPoolOauthSelection,
    selection,
    storage,
  );
}

export function readAccountValueVisibility(storage: RelayStorage | undefined = browserStorage()) {
  const visibilityValue = readMigratedRelayPreference(
    RELAY_STORAGE_KEYS.accountValueVisible,
    RELAY_STORAGE_KEYS.legacyPoolEconomicsVisible,
    "true",
    storage,
  );
  return visibilityValue !== "false";
}

export function writeAccountValueVisibility(
  visible: boolean,
  storage: RelayStorage | undefined = browserStorage(),
) {
  writeMigratedRelayPreference(
    RELAY_STORAGE_KEYS.accountValueVisible,
    RELAY_STORAGE_KEYS.legacyPoolEconomicsVisible,
    String(visible),
    storage,
  );
}

function readMigratedRelayPreference(
  currentKey: string,
  legacyKey: string,
  fallback: string,
  storage: RelayStorage | undefined,
) {
  try {
    const preferenceValue = storage?.getItem(currentKey) || storage?.getItem(legacyKey) || fallback;
    writeMigratedRelayPreference(currentKey, legacyKey, preferenceValue, storage);
    return preferenceValue;
  } catch {
    // A failed read must not replace unread preferences with a default.
    return fallback;
  }
}

function writeMigratedRelayPreference(
  currentKey: string,
  legacyKey: string,
  preferenceValue: string,
  storage: RelayStorage | undefined,
) {
  if (writeRelayPreference(currentKey, preferenceValue, storage)) {
    removeRelayPreference(legacyKey, storage);
  }
}

export function readLaunchApplicationAfterConnect(
  storage: RelayStorage | undefined = browserStorage(),
) {
  return readRelayPreference(RELAY_STORAGE_KEYS.launchApplicationAfterConnect, "1", storage) === "1";
}

export function writeLaunchApplicationAfterConnect(
  enabled: boolean,
  storage: RelayStorage | undefined = browserStorage(),
) {
  writeRelayPreference(RELAY_STORAGE_KEYS.launchApplicationAfterConnect, enabled ? "1" : "0", storage);
}

/**
 * This marker deliberately contains no session, account, or credential data.
 * It only lets the next launch report that a local import confirmation never
 * returned to the renderer, which distinguishes an abrupt app exit from a
 * regular per-item import failure.
 */
export function beginAccountImportConfirmation(storage: RelayStorage | undefined = browserStorage()) {
  writeRelayPreference(RELAY_STORAGE_KEYS.pendingAccountImportConfirmation, "1", storage);
}

export function finishAccountImportConfirmation(storage: RelayStorage | undefined = browserStorage()) {
  removeRelayPreference(RELAY_STORAGE_KEYS.pendingAccountImportConfirmation, storage);
}

export function consumeInterruptedAccountImportConfirmation(storage: RelayStorage | undefined = browserStorage()) {
  const pending = readRelayPreference(RELAY_STORAGE_KEYS.pendingAccountImportConfirmation, "", storage) === "1";
  if (pending) finishAccountImportConfirmation(storage);
  return pending;
}

function browserStorage(): RelayStorage | undefined {
  try {
    return globalThis.localStorage;
  } catch {
    return undefined;
  }
}
