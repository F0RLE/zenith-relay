import { gzipSync } from "node:zlib";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { startupAssets } from "./frontend-bundle-assets.mjs";

const distDirectory = resolve(import.meta.dirname, "../../src/dist");
const indexPath = join(distDirectory, "index.html");
const limits = {
  initialJavaScript: 150 * 1024,
  initialCss: 40 * 1024,
  // Includes lazy integration settings and debug context diagnostics.
  totalAssets: 375 * 1024,
  helpDocuments: 64 * 1024,
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
const htmlAssets = tags.flatMap((tag) => {
  const isScript = /^<script\b/i.test(tag);
  const isPreload = /^<link\b/i.test(tag) && /\brel=["'][^"']*\bmodulepreload\b[^"']*["']/i.test(tag);
  const isStylesheet = /^<link\b/i.test(tag) && /\brel=["'][^"']*\bstylesheet\b[^"']*["']/i.test(tag);
  const url = attribute(tag, isScript ? "src" : "href");
  return (isScript || isPreload || isStylesheet) && url ? [assetPath(url)] : [];
});
const manifest = JSON.parse(readFileSync(join(distDirectory, ".vite/manifest.json"), "utf8"));
const initialAssets = [...new Set([...htmlAssets, ...startupAssets(manifest).map((file) => assetPath(`/${file}`))])];

const initialJavaScript = initialAssets.filter((path) => path.endsWith(".js"));
const initialCss = initialAssets.filter((path) => path.endsWith(".css"));
const allAssets = collectAssets(distDirectory);
const helpDocuments = readdirSync(join(distDirectory, "assets"))
  .filter((file) => /^README-.*\.md$/.test(file))
  .map((file) => join(distDirectory, "assets", file));
if (helpDocuments.length !== 2) throw new Error("Both local Help languages must be bundled");
if (!initialJavaScript.length) throw new Error("Production entry script is missing from index.html");
const measurements = {
  initialJavaScript: initialJavaScript.reduce((total, path) => total + gzipSize(path), 0),
  initialCss: initialCss.reduce((total, path) => total + gzipSize(path), 0),
  totalAssets: allAssets.reduce((total, path) => total + gzipSize(path), 0),
  helpDocuments: helpDocuments.reduce((total, path) => total + gzipSize(path), 0),
};

const exceeded = [];
for (const [name, value] of Object.entries(measurements)) {
  const limit = limits[name];
  const difference = Math.abs(limit - value);
  console.log(`${name}: ${(value / 1024).toFixed(1)} KiB gzip (limit ${(limit / 1024).toFixed(1)} KiB; ${difference} bytes ${value > limit ? "over" : "remaining"})`);
  if (value > limit) exceeded.push(name);
}
if (exceeded.length) {
  console.error(`[bundle] Gzip limits exceeded: ${exceeded.join(", ")}. Reduce the affected output before building the native app.`);
  process.exitCode = 1;
}
