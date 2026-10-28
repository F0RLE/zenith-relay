import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { stableOutput } from "../build/stableOutput.mjs";

function fixture(run, { write = true } = {}) {
  const root = mkdtempSync(join(tmpdir(), "relay-stable-build-"));
  try {
    const dist = join(root, "dist");
    const asset = join(dist, "assets", "entry.js");
    mkdirSync(join(dist, "assets"), { recursive: true });
    writeFileSync(asset, "export const value = 1;");
    writeFileSync(join(dist, "index.html"), "<script src='assets/entry.js'></script>");
    for (const path of [asset, join(dist, "index.html"), join(dist, "assets"), dist]) utimesSync(path, 1000, 1000);
    const plugin = stableOutput();
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
