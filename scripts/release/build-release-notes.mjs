import { fileURLToPath } from "node:url";
import { buildReleaseNotes, readChangelogs } from "../lib/release-notes.mjs";

try {
  const [tag, ...extra] = process.argv.slice(2);
  if (extra.length || !/^v?\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(tag ?? "")) {
    throw new Error("Usage: bun scripts/release/build-release-notes.mjs <version-or-tag>");
  }
  const directory = fileURLToPath(new URL("../../docs/releases/", import.meta.url));
  console.log(buildReleaseNotes(readChangelogs(directory), tag.replace(/^v/, "")));
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
