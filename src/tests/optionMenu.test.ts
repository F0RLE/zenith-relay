import { describe, expect, test } from "bun:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { OptionMenu } from "../src/features/relay/components/ui/menus";

function renderMenu(value: string, options = [
  { value: "", label: "Automatic" },
  { value: "60", label: "One minute" },
]) {
  return renderToStaticMarkup(createElement(OptionMenu, {
    label: "Recovery delay", value, options, onChange: () => {},
  }));
}

describe("option menu selected values", () => {
  test("never presents an unknown saved value as the first option", () => {
    const html = renderMenu("120");
    expect(html).toContain('aria-label="Recovery delay: 120"');
    expect(html).not.toContain("Automatic");
  });

  test("keeps the label of a known option", () => {
    expect(renderMenu("60")).toContain('aria-label="Recovery delay: One minute"');
  });

  test("preserves the explicit empty default", () => {
    expect(renderMenu("")).toContain('aria-label="Recovery delay: Automatic"');
  });

  test("retains a saved value while its inventory is empty", () => {
    expect(renderMenu("saved-model", [])).toContain('aria-label="Recovery delay: saved-model"');
  });
});
