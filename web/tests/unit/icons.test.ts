import { describe, expect, test } from "bun:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import * as icons from "../../src/components/Icons";

describe("Hosted authored icon system", () => {
  test("all glyphs share a deterministic 20-unit outline grid", () => {
    const names = new Set<string>();
    for (const Icon of Object.values(icons)) {
      const svg = renderToStaticMarkup(createElement(Icon));
      expect(svg).toContain('viewBox="0 0 20 20"');
      expect(svg).toContain('stroke-width="1.5"');
      expect(svg).toContain('stroke="currentColor"');
      expect(svg).toContain('stroke-linecap="round"');
      expect(svg).toContain('stroke-linejoin="round"');
      expect(svg).toContain('aria-hidden="true"');
      expect(svg).toContain('focusable="false"');
      expect(svg).toMatch(/<(path|rect|circle|ellipse) /);
      expect(svg).not.toMatch(/<(text|image|foreignObject)\b|lucide/);
      const name = svg.match(/data-icon="([^"]+)"/)?.[1];
      expect(name).toBeTruthy();
      expect(names.has(name!)).toBe(false);
      names.add(name!);
    }
    expect(names.size).toBe(Object.keys(icons).length);
  });

  test("standalone status icons retain accessible names", () => {
    const svg = renderToStaticMarkup(createElement(icons.CircleCheck, { "aria-label": "Завершён" }));
    expect(svg).toContain('role="img"');
    expect(svg).toContain('aria-label="Завершён"');
    expect(svg).not.toContain('aria-hidden="true"');
  });

  test("callers retain their class, semantics and test hooks", () => {
    const svg = renderToStaticMarkup(createElement(icons.X, { className: "close-control", "aria-hidden": true, id: "close-icon" }));
    expect(svg).toContain('class="relay-icon close-control"');
    expect(svg).toContain('data-icon="close"');
    expect(svg).toContain('id="close-icon"');
    expect(svg).toContain('aria-hidden="true"');
  });
});
