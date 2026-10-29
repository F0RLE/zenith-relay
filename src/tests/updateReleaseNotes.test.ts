import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { localizeReleaseNotes, prepareReleaseNotes } from "../src/features/relay/shell/updateReleaseNotes";

const changelog = readFileSync(new URL("../../docs/releases/CHANGELOG.md", import.meta.url), "utf8");

function releaseSection(version: string): string {
  const section = changelog.split(/(?=^## \[)/m).find((entry) => entry.startsWith(`## [${version}]`));
  if (!section) throw new Error(`Missing changelog section: ${version}`);
  return section.trim();
}

function releaseNotesLayout(notes: string): string[] {
  return notes.split(/\r?\n/).flatMap((line) => {
    const heading = line.match(/^(#{3,4})\s/);
    return heading ? [heading[1]!] : line.startsWith("- ") ? ["-"] : [];
  });
}

function releaseNotesLinks(notes: string): string[] {
  return [...notes.matchAll(/\]\(([^)]+)\)/g)].map((match) => match[1]!).sort();
}

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

  test("keeps complete notes inside locale blocks in the changelog", () => {
    for (const version of ["Unreleased", "1.1.5", "1.1.4", "1.1.3"]) {
      const section = releaseSection(version);
      const markers = [...section.matchAll(/<!--\s*relay-notes:([a-z0-9-]+)\s*-->/gi)];
      expect(markers.map((marker) => marker[1]?.toLowerCase())).toEqual(["en", "ru"]);
      expect(section.slice(0, markers[0]!.index).trim()).toBe(section.split(/\r?\n/)[0]!);
      expect(localizeReleaseNotes(section, "en-US")).toContain("### Changed");
      expect(localizeReleaseNotes(section, "ru-RU")).toContain("### Изменения");
    }
  });

  test("keeps Unreleased translations aligned by topic, entries, and references", () => {
    const section = releaseSection("Unreleased");
    const english = localizeReleaseNotes(section, "en");
    const russian = localizeReleaseNotes(section, "ru");
    expect(english).toContain("### Fixed");
    expect(russian).toContain("### Исправления");
    expect(releaseNotesLayout(russian)).toEqual(releaseNotesLayout(english));
    expect(releaseNotesLinks(russian)).toEqual(releaseNotesLinks(english));
    for (const feature of ["Claude Desktop", "Responses", "Gemini", "BPS", "Cockpit Tools", "Moonshot"]) {
      expect(english).toContain(feature);
      expect(russian).toContain(feature);
    }
  });

  test("keeps the Excel client-header change in both 1.1.5 translations", () => {
    const section = releaseSection("1.1.5");
    const englishEntry = "Excel / Basis Points imports preserve allowlisted provider client headers";
    const russianEntry = "Импорт Excel / Basis Points сохраняет разрешённые заголовки клиента провайдера";
    expect(localizeReleaseNotes(section, "en")).toContain(englishEntry);
    expect(localizeReleaseNotes(section, "ru")).toContain(russianEntry);
    expect(releaseNotesLayout(localizeReleaseNotes(section, "ru")))
      .toEqual(releaseNotesLayout(localizeReleaseNotes(section, "en")));
    expect(releaseSection("Unreleased")).not.toContain(englishEntry);
    expect(releaseSection("Unreleased")).not.toContain(russianEntry);
  });
});
