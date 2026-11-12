import fs from "node:fs";

export function sameFileState(left, right) {
  return ["dev", "ino", "mode", "nlink", "size", "mtimeNs", "ctimeNs"].every(key => left[key] === right[key]);
}

export function fileChanged(path) {
  return Object.assign(new Error(`File changed while checking: ${path}`), { code: "ESTALE" });
}

/** Validate the open file handle to prevent path-replacement races. */
export function openCheckedFile(path, { directories = false, writable = false } = {}, io = fs) {
  const before = io.lstatSync(path, { bigint: true });
  if (before.isSymbolicLink() || (!before.isFile() && !(directories && before.isDirectory()))) return null;
  // A regular file can still alias an external object through a hard link.
  // Reading a chosen file is allowed; timestamp writes must not affect aliases.
  if (writable && before.isFile() && before.nlink !== 1n) return null;
  // Windows lacks O_NOFOLLOW/O_NONBLOCK. The descriptor identity check is
  // required on every platform; these flags additionally reject POSIX links/FIFOs.
  const access = writable && (before.isFile() || process.platform === "win32") ? fs.constants.O_RDWR : fs.constants.O_RDONLY;
  const flags = access | (fs.constants.O_NOFOLLOW ?? 0) | (fs.constants.O_NONBLOCK ?? 0);
  const fd = io.openSync(path, flags);
  try {
    const stat = io.fstatSync(fd, { bigint: true });
    if (!sameFileState(before, stat)) throw fileChanged(path);
    return { fd, stat };
  } catch (error) {
    io.closeSync(fd);
    throw error;
  }
}

export function readDescriptorText(fd, { maxBytes = Infinity } = {}, io = fs) {
  const size = io.fstatSync(fd).size;
  if (size > maxBytes) throw new Error("File exceeds the read limit");
  const buffer = Buffer.alloc(size + 1);
  let length = 0;
  while (length < buffer.length) {
    const count = io.readSync(fd, buffer, length, buffer.length - length, length);
    if (count === 0) break;
    length += count;
  }
  if (length > maxBytes) throw new Error("File exceeds the read limit");
  return buffer.toString("utf8", 0, length);
}

export function readCheckedText(path, io = fs, { maxBytes } = {}) {
  const opened = openCheckedFile(path, {}, io);
  if (!opened) return null;
  try {
    const text = maxBytes === undefined
      ? io.readFileSync(opened.fd, "utf8")
      : readDescriptorText(opened.fd, { maxBytes }, io);
    if (!sameFileState(opened.stat, io.fstatSync(opened.fd, { bigint: true }))) throw fileChanged(path);
    return text;
  } finally {
    io.closeSync(opened.fd);
  }
}
