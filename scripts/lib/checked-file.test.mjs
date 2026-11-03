import { expect, test } from "bun:test";
import fs from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { readCheckedText } from "./checked-file.mjs";

function fixture(run) {
  const root = fs.mkdtempSync(join(tmpdir(), "relay-checked-file-"));
  const path = join(root, "source.rs");
  fs.writeFileSync(path, "synthetic source");
  const handles = new Set();
  const io = {
    ...fs,
    openSync(...args) { const fd = fs.openSync(...args); handles.add(fd); return fd; },
    closeSync(fd) { fs.closeSync(fd); handles.delete(fd); },
  };
  try {
    run({ root, path, io, handles });
    expect(handles.size).toBe(0);
  } finally {
    for (const fd of handles) fs.closeSync(fd);
    fs.rmSync(root, { recursive: true, force: true });
  }
}

test("checked text reads a regular file and skips directories", () => fixture(({ root, path, io }) => {
  expect(readCheckedText(path, io)).toBe("synthetic source");
  expect(readCheckedText(root, io)).toBeNull();
}));

test("replacing a source after lstat cannot redirect the read", () => fixture(({ path, io }) => {
  const open = io.openSync;
  let reads = 0;
  io.openSync = (...args) => {
    fs.renameSync(path, `${path}.original`);
    fs.writeFileSync(path, "different source");
    return open(...args);
  };
  io.readFileSync = () => { reads++; throw new Error("must not read replacement"); };
  expect(() => readCheckedText(path, io)).toThrow("File changed while checking");
  expect(reads).toBe(0);
}));

test("changes during a read fail closed instead of checking stale source", () => fixture(({ path, io }) => {
  io.readFileSync = (fd, encoding) => {
    expect(typeof fd).toBe("number");
    const text = fs.readFileSync(fd, encoding);
    fs.writeFileSync(path, "changed source with additional content");
    return text;
  };
  expect(() => readCheckedText(path, io)).toThrow("File changed while checking");
}));

test("read and metadata errors close the owned handle and remain failures", () => {
  for (const method of ["readFileSync", "fstatSync"]) fixture(({ path, io }) => {
    io[method] = () => { throw new Error(`synthetic ${method} failure`); };
    expect(() => readCheckedText(path, io)).toThrow(`synthetic ${method} failure`);
  });
});

test("a directory link is never followed", () => fixture(({ root, path, io }) => {
  const link = join(root, "link.rs");
  fs.symlinkSync(root, link, process.platform === "win32" ? "junction" : "dir");
  expect(readCheckedText(link, io)).toBeNull();
  expect(fs.readFileSync(path, "utf8")).toBe("synthetic source");
}));
