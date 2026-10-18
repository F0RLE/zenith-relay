import { expect, test } from "bun:test";
import capability from "../../src-tauri/capabilities/default.json";

test("native titlebar dragging is permitted only for the main local window", () => {
  expect(capability.windows).toEqual(["main"]);
  expect(capability.permissions).toContain("core:window:allow-start-dragging");
  expect(capability).not.toHaveProperty("remote");
});
