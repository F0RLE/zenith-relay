import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import remarkParse from "remark-parse";
import { unified, type Plugin } from "unified";
import { remarkGfmReadOnly } from "../src/features/relay/components/remarkGfmReadOnly";

const examples = {
  gfm: [
    "# GFM reader",
    "~one~ and ~~two~~; https://example.invalid and demo@example.invalid.",
    "- [x] Done\n- [ ] Pending",
    "| Left | Right |\n| :--- | ---: |\n| `a\\|b` | **bold** |",
    "A footnote[^note].\n\n[^note]: Reference with *emphasis*.",
    "```ts\nconst value = '<b>text</b>';\n```",
  ].join("\n\n"),
  untrusted: '<script>alert(1)</script>\n\n[bad](javascript:alert%281%29)\n\n<img src="x" onerror="alert(1)">',
  english: readFileSync(new URL("../../docs/help/en/README.md", import.meta.url), "utf8"),
  russian: readFileSync(new URL("../../docs/help/ru/README.md", import.meta.url), "utf8"),
};

for (const [name, markdown] of Object.entries(examples)) {
  test(`read-only GFM preserves parsing and safe rendering: ${name}`, () => {
    const parse = (plugin: Plugin) => unified().use(remarkParse).use(plugin).parse(markdown);
    const render = (plugin: Plugin) => renderToStaticMarkup(createElement(ReactMarkdown, {
      skipHtml: true, remarkPlugins: [plugin], children: markdown,
    }));
    expect(parse(remarkGfmReadOnly)).toEqual(parse(remarkGfm));
    expect(render(remarkGfmReadOnly)).toBe(render(remarkGfm));
  });
}

test("read-only GFM does not register Markdown serialization", () => {
  const data = unified().use(remarkParse).use(remarkGfmReadOnly).freeze().data();
  expect(data.micromarkExtensions).toHaveLength(1);
  expect(data.fromMarkdownExtensions).toHaveLength(1);
  expect(data.toMarkdownExtensions).toBeUndefined();
});
