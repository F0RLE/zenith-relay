import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

export function parseChangelog(markdown) {
  if (/<!--\s*relay-notes:/i.test(markdown)) {
    throw new Error("Keep translations in separate changelog files, not locale blocks.");
  }
  const headings = [...markdown.matchAll(/^## \[([^\]\r\n]+)\][^\r\n]*$/gm)];
  const sections = new Map();
  for (const [index, heading] of headings.entries()) {
    const version = heading[1];
    if (sections.has(version)) throw new Error(`Duplicate changelog version: ${version}`);
    const body = markdown.slice(heading.index + heading[0].length, headings[index + 1]?.index).trim();
    sections.set(version, { heading: heading[0], body });
  }
  return sections;
}

export function readChangelogs(directory) {
  const sources = new Map();
  for (const filename of readdirSync(directory).sort()) {
    const match = filename.match(/^CHANGELOG(?:\.([a-z][a-z0-9]*(?:-[a-z0-9]+)*))?\.md$/i);
    if (!match) continue;
    const locale = (match[1] ?? "en").toLowerCase();
    if (sources.has(locale)) throw new Error(`Duplicate changelog locale: ${locale}`);
    sources.set(locale, parseChangelog(readFileSync(join(directory, filename), "utf8")));
  }
  if (!sources.has("en")) throw new Error("The English changelog is required.");
  return sources;
}

export function buildReleaseNotes(changelogs, version) {
  const english = changelogs.get("en")?.get(version);
  if (!english) throw new Error(`Missing English changelog version: ${version}`);
  const locales = ["en", ...[...changelogs.keys()].filter((locale) => locale !== "en").sort()];
  const blocks = locales.map((locale) => {
    if (!/^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$/.test(locale)) throw new Error(`Invalid changelog locale: ${locale}`);
    const section = changelogs.get(locale)?.get(version);
    if (!section?.body) throw new Error(`Missing or empty ${locale} release notes for ${version}`);
    if (section.heading !== english.heading) throw new Error(`Release heading/date mismatch for ${locale}: ${version}`);
    return `<!-- relay-notes:${locale} -->\n\n${section.body}`;
  });
  return `${english.heading}\n\n${blocks.join("\n\n")}`;
}
