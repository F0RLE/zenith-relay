import { createHash } from "node:crypto";
import { existsSync, lstatSync, readFileSync, readdirSync, utimesSync } from "node:fs";
import { join, resolve } from "node:path";

function snapshot(directory) {
  if (!existsSync(directory)) return null;
  const entries = new Map();
  function visit(path, relative) {
    const stat = lstatSync(path);
    // Do not inspect or restore timestamps outside the generated output tree.
    if (stat.isSymbolicLink() || (!stat.isFile() && !stat.isDirectory())) return false;
    entries.set(relative, {
      hash: stat.isDirectory() ? null : createHash("sha256").update(readFileSync(path)).digest("hex"),
      atime: stat.atimeMs / 1000,
      mtime: stat.mtimeMs / 1000,
    });
    if (stat.isDirectory()) {
      for (const name of readdirSync(path)) {
        if (!visit(join(path, name), join(relative, name))) return false;
      }
    }
    return true;
  }
  return visit(directory, "") ? entries : null;
}

/** Vite still rebuilds everything; byte-identical output need not relink Tauri. */
export function stableOutput() {
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
      before = writable ? snapshot(directory) : null;
    },
    writeBundle: {
      order: "post",
      handler() {
        if (!before) return;
        const after = snapshot(directory);
        if (!after || before.size !== after.size) return;
        for (const [path, entry] of before) {
          if (!after.has(path) || after.get(path).hash !== entry.hash) return;
        }
        // Restore only when every filename and byte matches, including public
        // assets and the manifest. Changed builds keep their new timestamps.
        for (const [path, entry] of [...before].reverse()) {
          utimesSync(join(directory, path), entry.atime, entry.mtime);
        }
        this.info("Output is byte-identical; preserved timestamps for Cargo reuse.");
      },
    },
  };
}
