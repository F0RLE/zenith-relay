import { describe, expect, test } from "bun:test";
import {
  connectionInitialView,
  connectionViews,
  reconcileRemoteConnectionView,
} from "../src/features/relay/pages/connections/connectionViewState";

describe("connection view state", () => {
  test("keeps the mode-specific set of available views", () => {
    expect(connectionViews("zenith", [])).toEqual(["sources"]);
    expect(connectionViews("local", [])).toEqual(["accounts", "sources", "proxies", "automations"]);
    expect(connectionViews("remote", ["accounts", "sources", "wake_tasks"])).toEqual(["accounts", "sources", "automations", "remote"]);
  });

  test("restores the requested sources view without carrying it into Zenith", () => {
    expect(connectionInitialView("local", "accounts", "sources")).toBe("sources");
    expect(connectionInitialView("local", "sources", null)).toBe("sources");
    expect(connectionInitialView("local", "accounts", null)).toBe("accounts");
    expect(connectionInitialView("zenith", "accounts", "sources")).toBe("sources");
  });

  test("falls back to remote while no remote runtime is connected", () => {
    expect(reconcileRemoteConnectionView("remote", true, "sources")).toBe("sources");
    expect(reconcileRemoteConnectionView("remote", false, "accounts")).toBe("remote");
    expect(reconcileRemoteConnectionView("local", true, "accounts")).toBe("accounts");
  });
});
