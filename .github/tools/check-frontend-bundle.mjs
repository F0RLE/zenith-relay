import { gzipSync } from "node:zlib";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";

const distDirectory = resolve(import.meta.dirname, "../../src/dist");
const indexPath = join(distDirectory, "index.html");
const limits = {
  initialJavaScript: 150 * 1024,
  initialCss: 40 * 1024,
  totalAssets: 360 * 1024,
};

function gzipSize(path) {
  return gzipSync(readFileSync(path), { level: 9 }).byteLength;
}

function assetPath(url) {
  const pathname = new URL(url, "https://bundle-check.invalid").pathname;
  const path = resolve(distDirectory, `.${decodeURIComponent(pathname)}`);
  if (relative(distDirectory, path).startsWith("..")) throw new Error(`Asset escapes dist: ${url}`);
  return path;
}

function collectAssets(directory) {
  return readdirSync(directory).flatMap((entry) => {
    const path = join(directory, entry);
    return statSync(path).isDirectory() ? collectAssets(path) : /\.(?:js|css)$/.test(path) ? [path] : [];
  });
}

function attribute(tag, name) {
  return tag.match(new RegExp(`\\b${name}=["']([^"']+)["']`, "i"))?.[1];
}

const html = readFileSync(indexPath, "utf8");
const tags = html.match(/<(?:script|link)\b[^>]*>/gi) ?? [];
const initialAssets = tags.flatMap((tag) => {
  const isModule = /^<script\b/i.test(tag) && /\btype=["']module["']/i.test(tag);
  const isPreload = /^<link\b/i.test(tag) && /\brel=["'][^"']*\bmodulepreload\b[^"']*["']/i.test(tag);
  const isStylesheet = /^<link\b/i.test(tag) && /\brel=["'][^"']*\bstylesheet\b[^"']*["']/i.test(tag);
  const url = attribute(tag, isModule ? "src" : "href");
  return (isModule || isPreload || isStylesheet) && url ? [assetPath(url)] : [];
});

const initialJavaScript = initialAssets.filter((path) => path.endsWith(".js"));
const initialCss = initialAssets.filter((path) => path.endsWith(".css"));
const allAssets = collectAssets(distDirectory);
if (!initialJavaScript.length) throw new Error("Production entry script is missing from index.html");
const measurements = {
  initialJavaScript: initialJavaScript.reduce((total, path) => total + gzipSize(path), 0),
  initialCss: initialCss.reduce((total, path) => total + gzipSize(path), 0),
  totalAssets: allAssets.reduce((total, path) => total + gzipSize(path), 0),
};

for (const [name, value] of Object.entries(measurements)) {
  const limit = limits[name];
  console.log(`${name}: ${(value / 1024).toFixed(1)} KiB gzip (limit ${(limit / 1024).toFixed(1)} KiB)`);
  if (value > limit) throw new Error(`${name} exceeds its gzip limit`);
}
