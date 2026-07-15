import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { en } from "../src/i18n/locales/en";
import { ru } from "../src/i18n/locales/ru";

describe("Relay translations", () => {
  test("English and Russian expose the same keys", () => {
    expect(flatten(en)).toEqual(flatten(ru));
  });

  test("every literal UI translation key exists", () => {
    const known = new Set(flatten(en));
    const used = new Set<string>();
    for (const file of walk(join(import.meta.dir, "..", "src"))) {
      const source = readFileSync(file, "utf8");
      for (const match of source.matchAll(/\bt\(\s*["']([^"']+)["']/g)) used.add(match[1]);
    }
    expect([...used].filter((key) => !known.has(key)).sort()).toEqual([]);
  });

  test("themed menus replace native select controls", () => {
    const nativeSelects = walk(join(import.meta.dir, "..", "src"))
      .filter((file) => readFileSync(file, "utf8").includes("<select"));
    expect(nativeSelects).toEqual([]);
  });
});

function flatten(value: object, prefix = "", result: string[] = []) {
  for (const [key, item] of Object.entries(value)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (item && typeof item === "object") flatten(item, path, result);
    else result.push(path);
  }
  return result.sort();
}

function walk(directory: string): string[] {
  return readdirSync(directory).flatMap((name) => {
    const path = join(directory, name);
    return statSync(path).isDirectory() ? walk(path) : path.endsWith(".tsx") ? [path] : [];
  });
}
