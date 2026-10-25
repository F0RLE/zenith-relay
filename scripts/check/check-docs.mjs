import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, realpathSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, isAbsolute, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

// Resolve the Markdown parser from the frontend's locked dependencies.
const requireFrontend = createRequire(new URL("../../src/package.json", import.meta.url));
const { unified } = await import(pathToFileURL(requireFrontend.resolve("unified")).href);
const { default: remarkParse } = await import(pathToFileURL(requireFrontend.resolve("remark-parse")).href);
const parser = unified().use(remarkParse);
const markdownFile = /\.(?:md|markdown)$/i;
const requiredDocuments = [
  "README.md", "AGENTS.md", "CONTRIBUTING.md", "LICENSE",
  "CONTRIBUTOR_LICENSE_AGREEMENT.md", ".github/PULL_REQUEST_TEMPLATE.md",
  "docs/README.md", "docs/project/PLANNING.md", "docs/project/ROADMAP.md", "docs/project/BASIS_POINTS.md",
  "docs/hosted/HOSTED_RELAY_ARCHITECTURE.md", "docs/hosted/HOSTED_RELAY_ROADMAP.md",
  "docs/hosted/HOSTED_RELAY_WEB_DESIGN.md", "docs/help/en/README.md",
  "docs/help/ru/README.md", "docs/releases/CHANGELOG.md", "catalog/README.md", "web/README.md",
];

function plainText(node) {
  if (node.type === "html") return "";
  if (node.type === "image") return node.alt ?? "";
  return node.children?.map(plainText).join("") ?? node.value ?? "";
}

function htmlEntities(value) {
  const named = { amp: "&", quot: '"', apos: "'", lt: "<", gt: ">" };
  return value.replace(/&(#x[\da-f]+|#\d+|amp|quot|apos|lt|gt);/gi, (original, entity) => {
    if (!entity.startsWith("#")) return named[entity.toLowerCase()];
    const hex = entity[1].toLowerCase() === "x";
    const number = Number.parseInt(entity.slice(hex ? 2 : 1), hex ? 16 : 10);
    return number > 0 && number <= 0x10ffff ? String.fromCodePoint(number) : original;
  });
}

export function inspectMarkdown(text) {
  const links = [];
  const anchors = new Set();
  const headingIds = new Set();
  function visit(node) {
    if (node.type === "heading") {
      const base = plainText(node).toLowerCase().replace(/[^\p{L}\p{N}\p{M}_\- ]/gu, "").replaceAll(" ", "-");
      let id = base;
      for (let index = 1; headingIds.has(id); index++) id = `${base}-${index}`;
      headingIds.add(id);
      anchors.add(id);
    }
    // Definitions cover full, collapsed and shortcut reference links without
    // misreading literal brackets, task checkboxes or code as references.
    if (["link", "image", "definition"].includes(node.type)) {
      links.push({ url: node.url, line: node.position.start.line });
    }
    if (node.type === "html") {
      const html = node.value.replace(/<!--[^]*?(?:-->|$)/g, (match) => match.replace(/[^\n]/g, " "));
      for (const tag of html.matchAll(/<([a-z][\w:-]*)\b(?:"[^"]*"|'[^']*'|[^'">])*>/gi)) {
        for (const attribute of tag[0].matchAll(/\s+([^\s"'<>/=]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+)))?/gi)) {
          const name = attribute[1].toLowerCase();
          if (!["href", "src", "id", "name"].includes(name) || attribute.slice(2).every((value) => value === undefined)) continue;
          const value = htmlEntities(attribute[2] ?? attribute[3] ?? attribute[4]);
          if (name === "id" || (name === "name" && tag[1].toLowerCase() === "a")) anchors.add(value);
          if (name === "href" || name === "src") {
            links.push({ url: value, line: node.position.start.line + html.slice(0, tag.index).split("\n").length - 1 });
          }
        }
      }
    }
    for (const child of node.children ?? []) visit(child);
  }
  visit(parser.parse(text));
  return { links, anchors };
}

export function listDocuments(root) {
  const result = spawnSync("git", ["ls-files", "--cached", "--others", "--exclude-standard", "-z"], {
    cwd: root, encoding: "utf8", windowsHide: true,
  });
  if (result.error || result.status !== 0) throw new Error(`Cannot list repository documentation: ${result.error?.message ?? result.stderr}`);
  return [...new Set(result.stdout.split("\0"))]
    .filter((path) => markdownFile.test(path) && existsSync(resolve(root, path)))
    .sort();
}

function outsideRoot(root, path) {
  const local = relative(root, path);
  return local === ".." || local.startsWith(`..${sep}`) || isAbsolute(local);
}

export function checkDocumentation(root, { files = listDocuments(root), required = requiredDocuments } = {}) {
  root = realpathSync(root);
  const errors = [];
  const documents = new Map();
  const directories = new Map();
  const counts = { documents: files.length, links: 0, anchors: 0, external: 0, optionalWorkspace: 0 };
  const document = (path) => {
    if (!documents.has(path)) documents.set(path, inspectMarkdown(readFileSync(path, "utf8")));
    return documents.get(path);
  };
  function localTarget(path) {
    if (outsideRoot(root, path)) return "target leaves the repository";
    // Check spelling on Windows too: a case-only typo must not pass locally
    // and fail when the same link is published or checked on Linux.
    let parent = root;
    for (const part of relative(root, path).split(sep).filter(Boolean)) {
      if (!directories.has(parent)) directories.set(parent, readdirSync(parent));
      if (!directories.get(parent).includes(part)) return "missing target or wrong filename case";
      parent = resolve(parent, part);
      if (outsideRoot(root, realpathSync(parent))) return "symlink target leaves the repository";
      if (parent !== path && !statSync(parent).isDirectory()) return "target parent is not a directory";
    }
    return undefined;
  }
  for (const path of required) {
    const target = resolve(root, path);
    const problem = localTarget(target);
    if (problem || !statSync(target).isFile()) errors.push(`${path}: required document missing (${problem ?? "not a file"})`);
  }
  if (!files.length) errors.push("No owned Markdown documents found.");
  for (const file of files) {
    const source = resolve(root, file);
    const sourceProblem = localTarget(source);
    if (sourceProblem) { errors.push(`${file}: ${sourceProblem}`); continue; }
    for (const { url, line } of document(source).links) {
      if (/^[a-z][a-z\d+.-]*:/i.test(url) || url.startsWith("//")) { counts.external++; continue; }
      // Workspace guidance is explicitly optional in AGENTS.md. Do not require
      // a parent checkout in CI, or silently exempt other escaping links.
      if (file === "AGENTS.md" && url === "../AGENTS.md") { counts.optionalWorkspace++; continue; }
      counts.links++;
      const fail = (message) => errors.push(`${file}:${line}: ${url} — ${message}`);
      let path;
      let fragment;
      try {
        const hash = url.indexOf("#");
        path = decodeURIComponent((hash < 0 ? url : url.slice(0, hash)).split("?")[0]);
        fragment = hash < 0 ? "" : decodeURIComponent(url.slice(hash + 1));
      } catch { fail("invalid percent encoding"); continue; }
      if (path.includes("\\") || path.includes("\0")) { fail("use a repository URL path with forward slashes"); continue; }
      const target = path ? resolve(path.startsWith("/") ? root : dirname(source), path.replace(/^\/+/, "")) : source;
      const problem = localTarget(target);
      if (problem) { fail(problem); continue; }
      if (fragment && markdownFile.test(target) && statSync(target).isFile()) {
        counts.anchors++;
        if (!document(target).anchors.has(fragment)) fail("missing heading or explicit HTML anchor");
      }
    }
  }
  return { ...counts, errors };
}

if (import.meta.main) {
  const root = fileURLToPath(new URL("../../", import.meta.url));
  try {
    const result = checkDocumentation(root);
    for (const error of result.errors) console.error(error);
    console.log(`Documentation: ${result.documents} files, ${result.links} local links, ${result.anchors} anchors; ${result.errors.length} errors. External URLs are not fetched; ${result.optionalWorkspace} optional workspace link(s) skipped.`);
    if (result.errors.length) process.exitCode = 1;
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
