import { expect, test } from "bun:test";
import { chmodSync, linkSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fixtureExists, fixtureOrigin, fixturePath, fixtureJourney, readFixture, writeFixture } from "../../../test-support/files.mjs";

test("browser fixture channels accept only bounded regular files in isolated roots", () => {
  const root = mkdtempSync(join(tmpdir(), "relay-hosted-"));
  chmodSync(root, 0o700);
  try {
    const directory = join(root, "exchange");
    mkdirSync(directory);
    writeFixture(directory, "ready", "synthetic");
    expect(fixtureExists(directory, "ready")).toBe(true);
    expect(readFixture(directory, "ready")).toBe("synthetic");
    expect(fixtureExists(directory, "missing")).toBe(false);
    expect(() => writeFixture(directory, "ready", "replace")).toThrow();
    expect(readFileSync(join(directory, "ready"), "utf8")).toBe("synthetic");
    for (const leaf of ["../escape", "/absolute", "nested/path", "..", "a\\b", "a\n"])
      expect(() => fixturePath(directory, leaf)).toThrow();
    expect(() => readFixture(join(tmpdir(), "unrelated"))).toThrow();
    expect(() => readFixture(directory)).toThrow();
    writeFileSync(join(directory, "large"), "x".repeat(65537));
    expect(() => readFixture(directory, "large")).toThrow();
    linkSync(join(directory, "ready"), join(directory, "alias"));
    expect(() => readFixture(directory, "alias")).toThrow();
    symlinkSync(directory, join(root, "linked"), process.platform === "win32" ? "junction" : "dir");
    expect(() => readFixture(join(root, "linked"), "ready")).toThrow();
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test("fixture URLs retain explicit loopback origins and reject credentials and redirects", () => {
  expect(fixtureOrigin("http://localhost:1443")).toBe("http://localhost:1443");
  expect(fixtureOrigin("https://localhost:1443", { secure: true })).toBe("https://localhost:1443");
  expect(fixtureOrigin("http://127.0.0.1:1443", { numeric: true })).toBe("http://127.0.0.1:1443");
  for (const origin of ["http://example.test:1443", "http://localhost:0", "http://localhost:1443/x",
    "http://user@localhost:1443", "http://localhost:1443?x", "http://localhost:1443#x", "file:///tmp/test"])
    expect(() => fixtureOrigin(origin)).toThrow();
});

test("customer fixtures explicitly select a known journey", () => {
  expect(fixtureJourney("funded")).toBe(true);
  expect(fixtureJourney("identity")).toBe(false);
  for (const name of [undefined, "", "typo", "__proto__", "constructor"]) expect(() => fixtureJourney(name)).toThrow();
});
