import { createHash } from "node:crypto";
import fs from "node:fs";
import { join, resolve } from "node:path";
import { fileChanged, openCheckedFile, sameFileState } from "../../scripts/lib/checked-file.mjs";

function closeSnapshot(entries, io) {
  for (const entry of entries?.values() ?? []) {
    if (entry.fd !== undefined) {
      io.closeSync(entry.fd);
      delete entry.fd;
    }
  }
}

function snapshot(directory, keepOpen, io) {
  const entries = new Map();
  function visit(path, relative) {
    const opened = openCheckedFile(path, { directories: true, writable: keepOpen }, io);
    if (!opened) throw fileChanged(path);
    const { stat, fd } = opened;
    const entry = { fd, stat, atime: Number(stat.atimeNs) / 1e9, mtime: Number(stat.mtimeNs) / 1e9 };
    entries.set(relative, entry);
    entry.hash = stat.isDirectory() ? null : createHash("sha256").update(io.readFileSync(fd)).digest("hex");
    if (stat.isDirectory()) {
      for (const name of io.readdirSync(path)) {
        visit(join(path, name), join(relative, name));
      }
    }
    if (!sameFileState(stat, io.fstatSync(fd, { bigint: true })) || !sameFileState(stat, io.lstatSync(path, { bigint: true }))) throw fileChanged(path);
  }
  try {
    visit(directory, "");
    if (!keepOpen) closeSnapshot(entries, io);
    return entries;
  } catch {
    // Timestamp reuse is optional. A missing, changing or unreadable tree must
    // retain fresh build timestamps, not fail the build or partially reuse it.
    closeSnapshot(entries, io);
    return null;
  }
}

/** Vite still rebuilds everything; byte-identical output need not relink Tauri. */
export function stableOutput(io = fs) {
  let directory = "";
  let before = null;
  let writable = false;
  return {
    name: "relay-stable-output",
    apply: "build",
    configResolved(config) {
      directory = resolve(config.root, config.build.outDir);
      writable = config.build.write !== false;
    },
    buildStart() {
      before = writable ? snapshot(directory, false, io) : null;
    },
    writeBundle: {
      order: "post",
      handler() {
        if (!before) return;
        const after = snapshot(directory, true, io);
        try {
          if (!after || before.size !== after.size) return;
          for (const [path, entry] of before) {
            const current = after.get(path);
            if (!current || current.hash !== entry.hash) return;
            if (!sameFileState(current.stat, io.fstatSync(current.fd, { bigint: true })) || !sameFileState(current.stat, io.lstatSync(join(directory, path), { bigint: true }))) return;
          }
          // Keep the validated post-build objects open through restoration.
          // Replaced pathnames must never redirect timestamp writes elsewhere.
          for (const [path, entry] of [...before].reverse()) {
            const current = after.get(path);
            if (!sameFileState(current.stat, io.fstatSync(current.fd, { bigint: true }))) return;
            io.futimesSync(current.fd, entry.atime, entry.mtime);
          }
          this.info("Output is byte-identical; preserved timestamps for Cargo reuse.");
        } finally {
          closeSnapshot(after, io);
        }
      },
    },
  };
}
