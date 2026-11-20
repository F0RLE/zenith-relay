import { closeSync, existsSync, fstatSync, linkSync, lstatSync, realpathSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { isAbsolute, join, relative, resolve, sep } from "node:path";
import { openCheckedFile, readDescriptorText, sameFileState } from "../../scripts/lib/checked-file.mjs";

// Rust creates and owns these private, disposable roots for child browser fixtures.
export function fixturePath(candidate, leaf) {
  if (typeof candidate !== "string" || !isAbsolute(candidate)) throw new Error("Expected an absolute fixture path");
  const temporary = realpathSync(tmpdir());
  const absolute = resolve(candidate);
  const parts = relative(temporary, absolute).split(sep);
  const rootName = parts.shift();
  if (!/^relay-hosted-[A-Za-z0-9_-]{6,}$/.test(rootName))
    throw new Error("Path is outside the isolated Hosted fixture");
  const root = join(temporary, rootName);
  let path = root;
  const rootStat = lstatSync(root);
  if (!rootStat.isDirectory() || rootStat.isSymbolicLink()
    || process.platform !== "win32" && (rootStat.mode & 0o077) !== 0)
    throw new Error("Expected a private fixture directory");
  if (leaf !== undefined) {
    if (!/^[A-Za-z0-9_-][A-Za-z0-9_.-]{0,95}$/.test(leaf)) throw new Error("Invalid fixture leaf");
    parts.push(leaf);
  }
  for (let i = 0; i < parts.length; i++) {
    const part = parts[i];
    if (!/^[A-Za-z0-9_.-]+$/.test(part) || part === "." || part === "..") throw new Error("Invalid fixture component");
    path = join(path, part);
    if (!path.startsWith(root + sep)) throw new Error("Expected a fixture child");
    try {
      const stat = lstatSync(path);
      if (stat.isSymbolicLink() || (i < parts.length - 1 ? !stat.isDirectory() : !stat.isFile() && !stat.isDirectory()))
        throw new Error("Fixture links and special files are not allowed");
    } catch (error) {
      if (error.code !== "ENOENT" || i !== parts.length - 1) throw error;
    }
  }
  if (!path.startsWith(root + sep)) throw new Error("Expected a fixture child");
  return path;
}

export function readFixture(candidate, leaf) {
  const path = fixturePath(candidate, leaf);
  const opened = openCheckedFile(path);
  if (!opened) throw new Error("Expected a regular fixture file");
  try {
    if (fstatSync(opened.fd).nlink !== 1) throw new Error("Fixture aliases are not allowed");
    const text = readDescriptorText(opened.fd, { maxBytes: 65536 });
    if (!sameFileState(opened.stat, fstatSync(opened.fd, { bigint: true }))) throw new Error("Fixture file changed during read");
    return text;
  } finally { closeSync(opened.fd); }
}

export const fixtureExists = (directory, leaf) => existsSync(fixturePath(directory, leaf));

export function writeFixture(directory, leaf, value = "") {
  const path = fixturePath(directory, leaf), partial = fixturePath(directory, leaf + ".partial");
  if (typeof value !== "string" || Buffer.byteLength(value) > 65536) throw new Error("Fixture exceeds the write limit");
  writeFileSync(partial, value, { flag: "wx", mode: 0o600 });
  try { linkSync(partial, path); } finally { unlinkSync(partial); }
}

export function fixtureOrigin(value, { secure = false, numeric = false } = {}) {
  const url = new URL(value);
  if (url.origin !== value || url.protocol !== (secure ? "https:" : "http:")
    || url.hostname !== (numeric ? "127.0.0.1" : "localhost") || !/^\d+$/.test(url.port)
    || Number(url.port) < 1 || Number(url.port) > 65535) throw new Error("Expected an isolated loopback origin");
  return `${secure ? "https" : "http"}://${numeric ? "127.0.0.1" : "localhost"}:${Number(url.port)}`;
}
