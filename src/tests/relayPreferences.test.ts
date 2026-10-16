import { describe, expect, test } from "bun:test";
import {
  RELAY_STORAGE_KEYS,
  readAccountValueVisibility,
  readCodexPoolOauthSelection,
  beginAccountImportConfirmation,
  consumeInterruptedAccountImportConfirmation,
  finishAccountImportConfirmation,
  readLaunchApplicationAfterConnect,
  readRelayPreference,
  removeRelayPreference,
  writeAccountValueVisibility,
  writeCodexPoolOauthSelection,
  writeLaunchApplicationAfterConnect,
  writeRelayPreference,
  type RelayStorage,
} from "../src/features/relay/state/relayPreferences";

function fakeStorage(initial: Record<string, string> = {}): RelayStorage & { values: Record<string, string> } {
  const values = { ...initial };
  return {
    values,
    getItem(key) {
      return values[key] ?? null;
    },
    setItem(key, value) {
      values[key] = value;
    },
    removeItem(key) {
      delete values[key];
    },
  };
}

describe("relay preferences", () => {
  test("reads, writes, and removes values through the storage boundary", () => {
    const storage = fakeStorage();

    expect(readRelayPreference("relay.mode", "local", storage)).toBe("local");
    expect(writeRelayPreference("relay.mode", "remote", storage)).toBe(true);
    expect(readRelayPreference("relay.mode", "local", storage)).toBe("remote");
    removeRelayPreference("relay.mode", storage);
    expect(readRelayPreference("relay.mode", "local", storage)).toBe("local");
  });

  test("falls back when browser storage throws", () => {
    const unavailable: RelayStorage = {
      getItem() {
        throw new Error("storage unavailable");
      },
      setItem() {
        throw new Error("storage unavailable");
      },
      removeItem() {
        throw new Error("storage unavailable");
      },
    };

    expect(readRelayPreference("relay.mode", "local", unavailable)).toBe("local");
    expect(readLaunchApplicationAfterConnect(unavailable)).toBe(true);
    expect(() => writeRelayPreference("relay.mode", "remote", unavailable)).not.toThrow();
    expect(writeRelayPreference("relay.mode", "remote", unavailable)).toBe(false);
    expect(() => removeRelayPreference("relay.mode", unavailable)).not.toThrow();
  });

  test("migrates the legacy Codex account selection once", () => {
    const storage = fakeStorage({
      [RELAY_STORAGE_KEYS.legacyCodexPoolOauthSelection]: "account_synthetic_2",
    });

    expect(readCodexPoolOauthSelection(storage)).toBe("account_synthetic_2");
    expect(storage.values[RELAY_STORAGE_KEYS.codexPoolOauthSelection]).toBe("account_synthetic_2");
    expect(storage.values[RELAY_STORAGE_KEYS.legacyCodexPoolOauthSelection]).toBeUndefined();
  });

  test("prefers the current Codex selection and removes the legacy key", () => {
    const storage = fakeStorage({
      [RELAY_STORAGE_KEYS.codexPoolOauthSelection]: "none",
      [RELAY_STORAGE_KEYS.legacyCodexPoolOauthSelection]: "account_stale",
    });

    expect(readCodexPoolOauthSelection(storage)).toBe("none");
    expect(storage.values[RELAY_STORAGE_KEYS.legacyCodexPoolOauthSelection]).toBeUndefined();
  });

  test("migrates account-value visibility from the percentage-calculation preference", () => {
    const storage = fakeStorage({
      [RELAY_STORAGE_KEYS.legacyPoolEconomicsVisible]: "false",
    });

    expect(readAccountValueVisibility(storage)).toBe(false);
    expect(storage.values[RELAY_STORAGE_KEYS.accountValueVisible]).toBe("false");
    expect(storage.values[RELAY_STORAGE_KEYS.legacyPoolEconomicsVisible]).toBeUndefined();

    writeAccountValueVisibility(true, storage);
    expect(storage.values[RELAY_STORAGE_KEYS.accountValueVisible]).toBe("true");
  });

  test("launches after connecting by default and persists an explicit opt-out", () => {
    const storage = fakeStorage();

    expect(readLaunchApplicationAfterConnect(storage)).toBe(true);
    expect(storage.values[RELAY_STORAGE_KEYS.launchApplicationAfterConnect]).toBeUndefined();
    writeLaunchApplicationAfterConnect(false, storage);
    expect(readLaunchApplicationAfterConnect(storage)).toBe(false);
    expect(storage.values[RELAY_STORAGE_KEYS.launchApplicationAfterConnect]).toBe("0");

    writeLaunchApplicationAfterConnect(true, storage);
    expect(readLaunchApplicationAfterConnect(storage)).toBe(true);
    expect(storage.values[RELAY_STORAGE_KEYS.launchApplicationAfterConnect]).toBe("1");
  });

  test("preserves an existing disabled launch preference when defaults change", () => {
    const storage = fakeStorage({ [RELAY_STORAGE_KEYS.launchApplicationAfterConnect]: "0" });
    expect(readLaunchApplicationAfterConnect(storage)).toBe(false);
    expect(storage.values[RELAY_STORAGE_KEYS.launchApplicationAfterConnect]).toBe("0");
  });

  test("reports only an import confirmation interrupted before a response", () => {
    const storage = fakeStorage();

    expect(consumeInterruptedAccountImportConfirmation(storage)).toBe(false);
    beginAccountImportConfirmation(storage);
    expect(consumeInterruptedAccountImportConfirmation(storage)).toBe(true);
    expect(consumeInterruptedAccountImportConfirmation(storage)).toBe(false);

    beginAccountImportConfirmation(storage);
    finishAccountImportConfirmation(storage);
    expect(consumeInterruptedAccountImportConfirmation(storage)).toBe(false);
  });

  test("migrates the removed automatic Codex selection to no account", () => {
    const storage = fakeStorage({ [RELAY_STORAGE_KEYS.codexPoolOauthSelection]: "auto" });

    expect(readCodexPoolOauthSelection(storage)).toBe("none");
    expect(storage.values[RELAY_STORAGE_KEYS.codexPoolOauthSelection]).toBe("none");
  });

  for (const migration of [
    {
      name: "Codex selection",
      currentKey: RELAY_STORAGE_KEYS.codexPoolOauthSelection,
      legacyKey: RELAY_STORAGE_KEYS.legacyCodexPoolOauthSelection,
      legacyValue: "account_synthetic_2",
      expectedValue: "account_synthetic_2",
      fallbackValue: "none",
      read: readCodexPoolOauthSelection,
      write: (storage: RelayStorage) => writeCodexPoolOauthSelection("none", storage),
      updatedValue: "none",
    },
    {
      name: "account-value visibility",
      currentKey: RELAY_STORAGE_KEYS.accountValueVisible,
      legacyKey: RELAY_STORAGE_KEYS.legacyPoolEconomicsVisible,
      legacyValue: "false",
      expectedValue: false,
      fallbackValue: true,
      read: readAccountValueVisibility,
      write: (storage: RelayStorage) => writeAccountValueVisibility(true, storage),
      updatedValue: "true",
    },
  ]) {
    test(`${migration.name} preserves the old value until migration can be written`, () => {
      const initial = { [migration.legacyKey]: migration.legacyValue };
      const storage = fakeStorage(initial);
      const persistValue = storage.setItem;
      storage.setItem = () => { throw new Error("writes blocked"); };

      expect(migration.read(storage)).toBe(migration.expectedValue);
      expect(storage.values).toEqual(initial);
      expect(migration.read(storage)).toBe(migration.expectedValue);

      storage.setItem = persistValue;
      expect(migration.read(storage)).toBe(migration.expectedValue);
      expect(storage.values).toEqual({ [migration.currentKey]: migration.legacyValue });
    });

    test(`${migration.name} does not overwrite unread preferences with a default`, () => {
      const initial = { [migration.legacyKey]: migration.legacyValue };
      const storage = fakeStorage(initial);
      const readValue = storage.getItem;
      storage.getItem = () => { throw new Error("reads blocked"); };

      expect(migration.read(storage)).toBe(migration.fallbackValue);
      expect(storage.values).toEqual(initial);

      storage.getItem = readValue;
      expect(migration.read(storage)).toBe(migration.expectedValue);
      expect(storage.values).toEqual({ [migration.currentKey]: migration.legacyValue });
    });

    test(`${migration.name} retains the previous preference after an explicit write fails`, () => {
      const initial = { [migration.legacyKey]: migration.legacyValue };
      const storage = fakeStorage(initial);
      const persistValue = storage.setItem;
      storage.setItem = () => { throw new Error("writes blocked"); };

      expect(() => migration.write(storage)).not.toThrow();
      expect(storage.values).toEqual(initial);
      expect(migration.read(storage)).toBe(migration.expectedValue);

      storage.setItem = persistValue;
      migration.write(storage);
      expect(storage.values).toEqual({ [migration.currentKey]: migration.updatedValue });
    });
  }
});
