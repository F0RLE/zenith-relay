import { expect, test } from "bun:test";
import { localeTemplate } from "./locale-template.mjs";

test("translation starters preserve parameters and use the target's plural forms", () => {
  const source = { title: "Example", values: ["{{value}}"], count_one: "{{count}} item", count_other: "{{count}} items" };
  const template = localeTemplate("ar", source);
  for (const category of ["zero", "one", "two", "few", "many", "other"]) expect(template[`count_${category}`]).toContain("{{count}}");
  expect(localeTemplate("ja", source)).toEqual({ title: "Example", values: ["{{value}}"], count_other: "{{count}} items" });
  expect(source).toEqual({ title: "Example", values: ["{{value}}"], count_one: "{{count}} item", count_other: "{{count}} items" });
});

test("translation starters reject malformed locale tags", () => {
  expect(() => localeTemplate("../invalid")).toThrow();
});
