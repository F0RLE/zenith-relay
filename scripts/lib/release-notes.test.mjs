import { describe, expect, test } from "bun:test";
import { fileURLToPath } from "node:url";
import { buildReleaseNotes, parseChangelog, readChangelogs } from "./release-notes.mjs";

const notes = (body = "- Synthetic change", heading = "## [1.2.3] - 2026-11-05") =>
  parseChangelog(`# Changelog\n\n${heading}\n\n${body}\n`);

describe("localized changelog compilation", () => {
  test("combines the requested version with English first and stable locale markers", () => {
    const sources = new Map([["ru", notes("- Translation")], ["en", notes()]]);
    expect(buildReleaseNotes(sources, "1.2.3")).toBe(
      "## [1.2.3] - 2026-11-05\n\n<!-- relay-notes:en -->\n\n- Synthetic change\n\n<!-- relay-notes:ru -->\n\n- Translation",
    );
  });

  test("does not merge versions or introductory text into release notes", () => {
    const source = "# Intro\n\n## [Unreleased]\n\n- Future\n\n## [1.2.3]\n\n- Current\n\n## [1.2.2]\n\n- Older\n";
    const compiled = buildReleaseNotes(new Map([["en", parseChangelog(source)]]), "1.2.3");
    expect(compiled).toContain("- Current");
    for (const unwanted of ["Intro", "Future", "Older"]) expect(compiled).not.toContain(unwanted);
  });

  test("rejects duplicate versions and embedded language blocks", () => {
    expect(() => parseChangelog("## [1.2.3]\n- A\n## [1.2.3]\n- B")).toThrow("Duplicate");
    expect(() => parseChangelog("## [1.2.3]\n<!-- relay-notes:en -->\n- A")).toThrow("separate");
  });

  test("fails closed on missing translations, empty notes and mismatched release dates", () => {
    expect(() => buildReleaseNotes(new Map([["ru", notes()]]), "1.2.3")).toThrow("English");
    for (const russian of [new Map(), notes("")]) {
      expect(() => buildReleaseNotes(new Map([["en", notes()], ["ru", russian]]), "1.2.3")).toThrow("Missing or empty ru");
    }
    expect(() => buildReleaseNotes(new Map([
      ["en", notes()], ["ru", notes("- Translation", "## [1.2.3] - 2026-11-04")],
    ]), "1.2.3")).toThrow("mismatch");
  });

  test("reads separate repository changelogs and compiles published translations", () => {
    const sources = readChangelogs(fileURLToPath(new URL("../../docs/releases/", import.meta.url)));
    expect([...sources.keys()].sort()).toEqual(["en", "ru"]);
    for (const version of ["Unreleased", "1.1.5", "1.1.4", "1.1.3"]) {
      const compiled = buildReleaseNotes(sources, version);
      expect(compiled).toContain("<!-- relay-notes:en -->");
      expect(compiled).toContain("<!-- relay-notes:ru -->");
    }
    for (const section of sources.get("en").values()) expect(section.body).not.toMatch(/\p{Script=Cyrillic}/u);
  });
});
