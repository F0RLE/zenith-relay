import { expect, test } from "bun:test";
import fs from "node:fs";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { stableOutput } from "../build/stableOutput.mjs";

function fixture(run, { write = true, io = fs } = {}) {
  const root = mkdtempSync(join(tmpdir(), "relay-stable-build-"));
  try {
    const dist = join(root, "dist");
    const asset = join(dist, "assets", "entry.js");
    mkdirSync(join(dist, "assets"), { recursive: true });
    writeFileSync(asset, "export const value = 1;");
    writeFileSync(join(dist, "index.html"), "<script src='assets/entry.js'></script>");
    for (const path of [asset, join(dist, "index.html"), join(dist, "assets"), dist]) utimesSync(path, 1000, 1000);
    const plugin = stableOutput(io);
    plugin.configResolved({ root, build: { outDir: "dist", write } });
    plugin.buildStart();
    const info = [];
    const finish = () => plugin.writeBundle.handler.call({ info: message => info.push(message) });
    run({ root, dist, asset, finish, info, plugin });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

test("identical output preserves file and directory timestamps without skipping a build", () => fixture(({ dist, asset, finish, info }) => {
  writeFileSync(asset, "export const value = 1;");
  utimesSync(dist, 2000, 2000);
  finish();
  expect(statSync(asset).mtimeMs).toBe(1000000);
  expect(statSync(dist).mtimeMs).toBe(1000000);
  expect(info).toHaveLength(1);
}));

test("changed content keeps fresh timestamps even when size is unchanged", () => fixture(({ asset, finish, info }) => {
  writeFileSync(asset, "export const value = 2;");
  finish();
  expect(statSync(asset).mtimeMs).toBeGreaterThan(1000000);
  expect(readFileSync(asset, "utf8")).toContain("2");
  expect(info).toHaveLength(0);
}));

test("added and removed assets invalidate the whole snapshot", () => {
  for (const remove of [false, true]) fixture(({ dist, asset, finish, info }) => {
    if (remove) rmSync(asset); else writeFileSync(join(dist, "extra.js"), "new");
    utimesSync(dist, 2000, 2000);
    finish();
    expect(statSync(dist).mtimeMs).toBe(2000000);
    expect(info).toHaveLength(0);
  });
});

test("write:false experiments never restore output timestamps", () => fixture(({ asset, finish, info }) => {
  utimesSync(asset, 2000, 2000);
  finish();
  expect(statSync(asset).mtimeMs).toBe(2000000);
  expect(info).toHaveLength(0);
}, { write: false }));

test("a clean rebuilt output uses new objects, not pre-build handles", () => fixture(({ dist, asset, finish, info }) => {
  rmSync(dist, { recursive: true });
  mkdirSync(join(dist, "assets"), { recursive: true });
  writeFileSync(asset, "export const value = 1;");
  writeFileSync(join(dist, "index.html"), "<script src='assets/entry.js'></script>");
  finish();
  expect(statSync(asset).mtimeMs).toBe(1000000);
  expect(statSync(dist).mtimeMs).toBe(1000000);
  expect(info).toHaveLength(1);
}));

test("replacement between metadata inspection and open disables reuse", () => {
  let replace;
  const io = { ...fs, openSync(path, flags) { replace?.(path); return fs.openSync(path, flags); } };
  fixture(({ root, asset, finish, info }) => {
    const outside = join(root, "outside.js");
    writeFileSync(outside, "export const value = 1;");
    utimesSync(outside, 2000, 2000);
    replace = path => {
      if (path !== asset) return;
      replace = undefined;
      fs.renameSync(asset, `${asset}.original`);
      fs.linkSync(outside, asset);
    };
    finish();
    expect(statSync(outside).mtimeMs).toBe(2000000);
    expect(info).toHaveLength(0);
  }, { io });
});

test("replacement immediately before restoration cannot redirect timestamp writes", () => {
  let replace;
  const io = { ...fs, futimesSync(fd, atime, mtime) { replace?.(); fs.futimesSync(fd, atime, mtime); } };
  fixture(({ root, asset, finish }) => {
    const outside = join(root, "outside.js");
    writeFileSync(outside, "export const value = 1;");
    utimesSync(outside, 2000, 2000);
    replace = () => {
      replace = undefined;
      fs.renameSync(asset, `${asset}.original`);
      fs.linkSync(outside, asset);
    };
    finish();
    expect(statSync(outside).mtimeMs).toBe(2000000);
  }, { io });
});

test("all descriptors close on success, changed output and snapshot failure", () => {
  for (const scenario of ["success", "changed", "unreadable"]) {
    const handles = new Set();
    let failRead = false;
    const io = {
      ...fs,
      openSync(...args) { const fd = fs.openSync(...args); handles.add(fd); return fd; },
      closeSync(fd) { fs.closeSync(fd); handles.delete(fd); },
      readFileSync(...args) { if (failRead) throw new Error("synthetic read failure"); return fs.readFileSync(...args); },
    };
    fixture(({ asset, finish, info }) => {
      expect(handles.size).toBe(0);
      if (scenario === "changed") writeFileSync(asset, "changed");
      failRead = scenario === "unreadable";
      finish();
      expect(handles.size).toBe(0);
      expect(info).toHaveLength(scenario === "success" ? 1 : 0);
    }, { io });
  }
});

test("directory links disable timestamp reuse", () => fixture(({ root, dist, finish, info }) => {
  fs.symlinkSync(root, join(dist, "link"), process.platform === "win32" ? "junction" : "dir");
  finish();
  expect(info).toHaveLength(0);
}));

test("preexisting hard links never reuse timestamps on an external inode", () => fixture(({ root, asset, finish, info }) => {
  const outside = join(root, "outside.js");
  writeFileSync(outside, "export const value = 1;");
  utimesSync(outside, 2000, 2000);
  rmSync(asset);
  fs.linkSync(outside, asset);
  finish();
  expect(statSync(outside).mtimeMs).toBe(2000000);
  expect(info).toHaveLength(0);
}));
