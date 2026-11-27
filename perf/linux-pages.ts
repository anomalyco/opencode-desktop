// Linux: a file's pages out of, or into, the page cache. A launch whose exe is not in memory reads it
// from the disk (the first launch after a boot or an update); a copy made a moment ago is still in
// memory, so the launch bench drops it by hand.
import { dlopen, FFIType } from "bun:ffi"
import { closeSync, fsyncSync, openSync, readSync } from "node:fs"

const POSIX_FADV_DONTNEED = 4

/** Drops the file's pages from the page cache (`posix_fadvise`: needs no rights beyond reading the
 *  file). Pages another process has mapped stay: close the app's other copies first. */
export function evict(path: string) {
  const libc = dlopen("libc.so.6", { posix_fadvise: { args: [FFIType.i32, FFIType.i64, FFIType.i64, FFIType.i32], returns: FFIType.i32 } }).symbols
  const fd = openSync(path, "r")
  // Written pages are dropped only once they are on the disk.
  fsyncSync(fd)
  const failed = libc.posix_fadvise(fd, 0n, 0n, POSIX_FADV_DONTNEED)
  closeSync(fd)
  if (failed) throw new Error(`posix_fadvise(${path}) failed: ${failed}`)
}

/** Reads the whole file, as a launch a moment ago leaves it: in the page cache. */
export function warm(path: string) {
  const fd = openSync(path, "r")
  const chunk = new Uint8Array(4 << 20)
  while (readSync(fd, chunk, 0, chunk.length, null) > 0);
  closeSync(fd)
}
