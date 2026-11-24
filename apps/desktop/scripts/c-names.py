#!/usr/bin/env python3
"""Makes the hidden names of the C code in static archives visible, for one of the app's code libraries.

    c-names.py <folder for the copies> <the linker's arguments...>     prints the arguments, one a line

link-order.sh runs it for a link of libopencode_layer_*.dylib (build-app.sh, OPENCODE_RUNE_LAYERS).
Some C libraries are compiled with hidden names (zstd: -fvisibility=hidden), which is right for
one exe and wrong here: Rust's generic wrappers around them are compiled into the crate that uses
them, which may be in a higher library, and a hidden name cannot be bound from another file. For
every archive among the arguments (.rlib, .a) that holds such objects, a copy is written with the
"private extern" bit of their defined names cleared, and the argument names the copy. Rust's own
objects (*.rcgu.o) are left alone: rustc decides what a library exports of those.

An archive that holds C objects is also loaded whole (-force_load): the linker takes from an
archive only what the link itself calls, and a C function that only a higher library calls would
not be in the file. Rust's compiler_builtins is left as it is: every file links its own.
"""
import os
import struct
import sys

N_STAB, N_PEXT, N_TYPE, N_EXT = 0xE0, 0x10, 0x0E, 0x01
LC_SYMTAB = 0x2
MH_MAGIC_64 = 0xFEEDFACF


def globalize(data, start, size):
    """Clears N_PEXT on the defined external names of the Mach-O object at data[start:start+size]; returns how many."""
    if size < 32 or struct.unpack_from("<I", data, start)[0] != MH_MAGIC_64:
        return None
    ncmds = struct.unpack_from("<I", data, start + 16)[0]
    at, changed = start + 32, 0
    for _ in range(ncmds):
        cmd, cmdsize = struct.unpack_from("<II", data, at)
        if cmd == LC_SYMTAB:
            symoff, nsyms = struct.unpack_from("<II", data, at + 8)
            for n in range(nsyms):
                where = start + symoff + n * 16 + 4
                kind = data[where]
                if not kind & N_STAB and kind & N_EXT and kind & N_PEXT and kind & N_TYPE:
                    data[where] = kind & ~N_PEXT
                    changed += 1
        at += cmdsize
    return changed


def patch(path, folder):
    """Returns the archive to link (a copy when names changed) and whether it holds C objects."""
    with open(path, "rb") as f:
        data = bytearray(f.read())
    if data[:8] != b"!<arch>\n" or os.path.basename(path).startswith("libcompiler_builtins-"):
        return path, False
    at, changed, objects = 8, 0, 0
    while at + 60 <= len(data):
        name = bytes(data[at : at + 16]).decode("latin-1").rstrip()
        size = int(bytes(data[at + 48 : at + 58]).decode("latin-1").strip() or "0")
        body, length = at + 60, size
        if name.startswith("#1/"):
            extra = int(name[3:])
            name = bytes(data[body : body + extra]).decode("latin-1").rstrip("\0")
            body, length = body + extra, size - extra
        # A name this does not read (the GNU form) is taken for Rust's and left alone.
        if not name.endswith(".rcgu.o") and not name.startswith(("__.SYMDEF", "/")) and not name.endswith(".rmeta"):
            names = globalize(data, body, length)
            if names is not None:
                objects += 1
                changed += names
        at += 60 + size + (size & 1)
    if not changed:
        return path, objects > 0
    os.makedirs(folder, exist_ok=True)
    copy = os.path.join(folder, os.path.basename(path))
    with open(copy, "wb") as f:
        f.write(data)
    print("c-names.py: %s: %d hidden C names made visible" % (os.path.basename(path), changed), file=sys.stderr)
    return copy, True


def main():
    folder, args = sys.argv[1], sys.argv[2:]
    done = {}
    for arg in args:
        path = arg[len("-Wl,-force_load,") :] if arg.startswith("-Wl,-force_load,") else arg
        if path.endswith((".rlib", ".a")) and os.path.isfile(path):
            if path in done:
                # Named twice: once is enough for an archive that is loaded whole.
                if done[path][1]:
                    continue
            else:
                done[path] = patch(path, folder)
            arg = "-Wl,-force_load," + done[path][0] if done[path][1] else arg.replace(path, done[path][0])
        print(arg)


main()
