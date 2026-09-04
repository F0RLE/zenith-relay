import { describe, expect, test } from "bun:test";
import { localizeReleaseNotes, prepareReleaseNotes } from "../src/features/relay/shell/updateReleaseNotes";

const localizedNotes = [
  "Downloads that are not part of the in-app notes",
  "<!-- relay-notes:en -->",
  "## [1.1.2] - 2026-09-03",
  "",
  "### Routing",
  "",
  "- Faster fallback",
  "<!-- relay-notes:ru -->",
  "## [1.1.2] - 2026-09-03",
  "",
  "### Маршрутизация",
  "",
  "- Ускорено переключение",
].join("\n");

describe("update release notes", () => {
  test("selects the exact or base locale section", () => {
    expect(localizeReleaseNotes(localizedNotes, "ru-RU")).toContain("Ускорено переключение");
    expect(localizeReleaseNotes(localizedNotes, "en-US")).toContain("Faster fallback");
  });

  test("falls back to English when the active locale is unavailable", () => {
    expect(localizeReleaseNotes(localizedNotes, "zh-CN")).toContain("Faster fallback");
  });

  test("preserves unmarked release notes", () => {
    expect(localizeReleaseNotes("## Changes\n\n- Fixed updates", "ru")).toBe("## Changes\n\n- Fixed updates");
  });

  test("removes only the redundant leading version heading", () => {
    expect(prepareReleaseNotes(localizedNotes, "ru", "1.1.2")).toBe("### Маршрутизация\n\n- Ускорено переключение");
    expect(prepareReleaseNotes("## Important changes\n\n- Fixed updates", "en", "1.1.2")).toBe("## Important changes\n\n- Fixed updates");
  });
});
