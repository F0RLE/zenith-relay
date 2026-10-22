import { describe, expect, test } from "bun:test";
import {
  connectionInitialView,
  connectionViews,
} from "../src/features/relay/pages/connections/connectionViewState";

describe("connection view state", () => {
  test("offers only local connection views without the retired remote page", () => {
    expect(connectionViews()).toEqual(["accounts", "sources", "proxies", "automations", "trash"]);
  });

  test("restores the requested sources view", () => {
    expect(connectionInitialView("accounts", "sources")).toBe("sources");
    expect(connectionInitialView("sources", null)).toBe("sources");
    expect(connectionInitialView("accounts", null)).toBe("accounts");
  });

  test("ignores saved requests to the retired server page", () => {
    expect(connectionInitialView("accounts", "remote")).toBe("accounts");
    expect(connectionInitialView("sources", "remote")).toBe("sources");
  });
});
