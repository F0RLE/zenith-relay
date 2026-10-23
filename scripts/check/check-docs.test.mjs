import { afterEach, describe, expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { checkDocumentation, inspectMarkdown, listDocuments } from "./check-docs.mjs";

const temporaryRoots = [];
function fixture(files) {
  const root = mkdtempSync(join(tmpdir(), "relay-docs-test-"));
  temporaryRoots.push(root);
  for (const [path, content] of Object.entries(files)) {
    const target = join(root, path);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, content);
  }
  return root;
}
afterEach(() => {
  for (const root of temporaryRoots.splice(0)) rmSync(root, { recursive: true, force: true });
});
const check = (root, files, required = []) => checkDocumentation(root, { files, required });

describe("Markdown structure", () => {
  test("parses inline, reference, image and HTML links with parentheses, titles and entities", () => {
    const result = inspectMarkdown([
      '[one](guide(a).md#hello "A title")', '[two][target]', '[target]: <space file.md>',
      '![image](image.png)', '<a href="guide.md?x=1&amp;y=2" id="manual">link</a>',
      '<img src="image.png">',
    ].join("\n\n"));
    expect(result.links.map((link) => link.url)).toEqual([
      "guide(a).md#hello", "space file.md", "image.png", "guide.md?x=1&y=2", "image.png",
    ]);
    expect(result.anchors.has("manual")).toBe(true);
  });

  test("ignores code, escaped examples and commented-out links", () => {
    const source = [
      '```md', '[bad](missing.md)', '```', '~~~', '[bad](missing.md)', '~~~',
      '', '    [bad](missing.md)', '', '`[bad](missing.md)`',
      '\\[bad](missing.md)', '<!-- <a href="missing.md">hidden</a> -->',
    ].join("\n");
    expect(inspectMarkdown(source).links).toEqual([]);
  });

  test("does not mistake data attributes or quoted prose for HTML destinations", () => {
    const result = inspectMarkdown('<img alt="a > b; href=not-a-link" data-src="not-a-link.png" src="image.png">');
    expect(result.links.map((link) => link.url)).toEqual(['image.png']);
  });

  test("supports formatted and Unicode headings, setext headings and duplicate slugs", () => {
    const result = inspectMarkdown([
      '# **Привет** `API`', '## Excel / Basis Points', '## repeat', '## repeat-1',
      '## repeat', '', 'Setext heading', '--------------',
    ].join("\n"));
    expect([...result.anchors]).toEqual([
      "привет-api", "excel--basis-points", "repeat", "repeat-1", "repeat-2", "setext-heading",
    ]);
  });
});

describe("local documentation links", () => {
  test("resolves relative/root links, query strings, encoded spaces and anchors", () => {
    const root = fixture({
      'README.md': '[guide](docs/space%20file.md?view=1#%D0%BF%D1%80%D0%B8%D0%B2%D0%B5%D1%82)\n![img](image.png)',
      'docs/space file.md': '# Привет\n[home](/README.md)\n[here](#привет)',
      'image.png': 'synthetic image fixture',
    });
    const result = check(root, ['README.md', 'docs/space file.md']);
    expect(result.errors).toEqual([]);
    expect(result.links).toBe(4);
    expect(result.anchors).toBe(2);
  });

  test("reports missing files and anchors with the source line", () => {
    const root = fixture({ 'README.md': '# Home\n[bad](absent.md)\n[bad](#absent)' });
    const result = check(root, ['README.md']);
    expect(result.errors).toHaveLength(2);
    expect(result.errors[0]).toContain('README.md:2: absent.md');
    expect(result.errors[1]).toContain('README.md:3: #absent');
  });

  test("rejects filename case mismatches on every platform", () => {
    const root = fixture({ 'README.md': '[bad](guide.md)', 'Guide.md': '# Guide' });
    expect(check(root, ['README.md']).errors[0]).toContain('wrong filename case');
  });

  test("limits the optional parent link to the workspace instruction link", () => {
    const root = fixture({ 'AGENTS.md': '[workspace](../AGENTS.md)', 'README.md': '[bad](../AGENTS.md)' });
    const result = check(root, ['AGENTS.md', 'README.md']);
    expect(result.optionalWorkspace).toBe(1);
    expect(result.errors).toHaveLength(1);
    expect(result.errors[0]).toContain('target leaves the repository');
  });

  test("does not fetch external URLs or require a live network", () => {
    const root = fixture({ 'README.md': '[web](https://example.invalid/x)\n[mail](mailto:example@example.invalid)' });
    const result = check(root, ['README.md']);
    expect(result.errors).toEqual([]);
    expect(result.external).toBe(2);
    expect(result.links).toBe(0);
  });

  test("rejects malformed encoding and nonexistent nested paths without throwing", () => {
    const root = fixture({ 'README.md': '[bad](bad%ZZ.md)\n[bad](absent/sub/file.md)' });
    expect(check(root, ['README.md']).errors).toHaveLength(2);
  });

  test("requires owned entry documents and a nonempty inventory", () => {
    const root = fixture({ 'README.md': '# Home' });
    expect(check(root, ['README.md'], ['README.md', 'AGENTS.md']).errors[0]).toContain('AGENTS.md: required document missing');
    expect(check(root, []).errors).toEqual(['No owned Markdown documents found.']);
  });

  test("checks HTML anchors and local image targets", () => {
    const root = fixture({ 'README.md': '<a name="manual"></a>\n\n[ok](#manual)\n<img src="absent.png">' });
    const result = check(root, ['README.md']);
    expect(result.anchors).toBe(1);
    expect(result.errors).toHaveLength(1);
    expect(result.errors[0]).toContain('absent.png');
  });

  test("includes tracked and new documents, but excludes ignored generated files", () => {
    const root = fixture({ '.gitignore': 'generated/\n', 'README.md': '# Home', 'new.md': '# New', 'generated/bad.md': '[bad](missing.md)' });
    execFileSync('git', ['init', '--quiet'], { cwd: root, windowsHide: true });
    execFileSync('git', ['add', '--', 'README.md'], { cwd: root, windowsHide: true });
    expect(listDocuments(root)).toEqual(['README.md', 'new.md']);
    expect(check(root, listDocuments(root)).errors).toEqual([]);
  });
});
