import fs from "node:fs";

export function sameFileState(left, right) {
  return ["dev", "ino", "mode", "nlink", "size", "mtimeNs", "ctimeNs"].every(key => left[key] === right[key]);
}

export function fileChanged(path) {
  return Object.assign(new Error(`File changed while checking: ${path}`), { code: "ESTALE" });
}

/** Pin a descriptor without creation/truncation; validate it before any read. */
export function openCheckedFile(path, io = fs, { directories = false, writable = false } = {}) {
  // Windows lacks O_NOFOLLOW/O_NONBLOCK. The descriptor identity check is
  // required on every platform; these flags additionally reject POSIX links/FIFOs.
  const access = writable && process.platform === "win32" ? fs.constants.O_RDWR : fs.constants.O_RDONLY;
  const flags = access | (fs.constants.O_NOFOLLOW ?? 0) | (fs.constants.O_NONBLOCK ?? 0);
  let fd;
  try { fd = io.openSync(path, flags, 0o600); }
  catch (error) {
    if (error.code === "ELOOP" || error.code === "EISDIR") return null;
    if (error.code === "EACCES" || error.code === "EPERM") {
      const stat = io.lstatSync(path);
      if (stat.isDirectory() || stat.isSymbolicLink()) return null;
    }
    throw error;
  }
  let retained = false;
  try {
    const stat = io.fstatSync(fd, { bigint: true });
    const current = io.lstatSync(path, { bigint: true });
    if (current.isSymbolicLink() || (!stat.isFile() && !(directories && stat.isDirectory()))) return null;
    if (writable && stat.isFile() && stat.nlink !== 1n) return null;
    if (!sameFileState(current, stat)) throw fileChanged(path);
    retained = true;
    return { fd, stat };
  } finally { if (!retained) io.closeSync(fd); }
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
  const opened = openCheckedFile(path, io);
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
