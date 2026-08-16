import { describe, expect, test } from "bun:test";
import {
  RELAY_STORAGE_KEYS,
  readCodexPoolOauthSelection,
  readRelayPreference,
  removeRelayPreference,
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
    writeRelayPreference("relay.mode", "remote", storage);
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
    expect(() => writeRelayPreference("relay.mode", "remote", unavailable)).not.toThrow();
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
});
