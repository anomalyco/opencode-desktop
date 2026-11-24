"""Rewrites, in place, the names a Mach-O file takes from one library to that library's short aliases.

A file names what it takes from another in its chained fixups (LC_DYLD_CHAINED_FIXUPS): a table of imports, each
a library ordinal and the offset of a NUL-ended name in a pool. dyld finds each name in the library's export trie at
every launch, and the walk costs by the name's length (a Rust v0 name is 100 characters and more). The short name
is written over the start of the long one in the pool, so no offset or size changes.

    short_names.py <file> <library file name> <alias list: lines of "long short">
"""
import struct
import sys

LOADS = (0xC, 0x80000018, 0x8000001F, 0x20, 0x80000023)  # the commands that take a library ordinal, in order
FIXUPS = 0x80000034


def rename_imports(path, library, short):
    """Returns how many imports were renamed. Raises when the file is not what this understands."""
    with open(path, "rb") as f:
        data = bytearray(f.read())
    magic, _, _, _, count, _ = struct.unpack_from("<IiiIII", data, 0)
    if magic != 0xFEEDFACF:
        raise ValueError("%s is not a 64-bit Mach-O file of one architecture" % path)
    at, ordinal, wanted, blob = 32, 0, None, None
    for _ in range(count):
        cmd, size = struct.unpack_from("<II", data, at)
        if cmd in LOADS:
            ordinal += 1
            name_at = at + struct.unpack_from("<I", data, at + 8)[0]
            if data[name_at:data.index(b"\0", name_at)].decode().endswith("/" + library):
                wanted = ordinal
        elif cmd == FIXUPS:
            blob = struct.unpack_from("<II", data, at + 8)
        at += size
    if wanted is None or blob is None:
        raise ValueError("%s does not link %s, or has no chained fixups" % (path, library))
    base = blob[0]
    _, _, imports_at, symbols_at, imports, form, packed = struct.unpack_from("<7I", data, base)
    if packed != 0 or form not in (1, 2, 3):
        raise ValueError("%s: import format %d / symbol format %d is not handled" % (path, form, packed))
    step = {1: 4, 2: 8, 3: 16}[form]
    entries = []
    for i in range(imports):
        if form == 3:
            raw = struct.unpack_from("<Q", data, base + imports_at + i * step)[0]
            entries.append((raw & 0xFFFF, raw >> 32))
        else:
            raw = struct.unpack_from("<I", data, base + imports_at + i * step)[0]
            entries.append((raw & 0xFF, raw >> 9))
    pool = base + symbols_at
    ends = {}
    for _, offset in entries:
        if offset not in ends:
            ends[offset] = data.index(b"\0", pool + offset) - pool
    starts = sorted(ends)
    done = 0
    renamed = set()
    for lib, offset in entries:
        if lib != wanted:
            continue
        if offset in renamed:  # a name two imports share, already rewritten
            done += 1
            continue
        name = data[pool + offset:pool + ends[offset]].decode()
        if name not in short:
            raise ValueError("%s takes %s from %s, which has no short name" % (path, name, library))
        new = short[name].encode() + b"\0"
        if len(new) > ends[offset] - offset + 1:
            raise ValueError("the short name of %s is longer than it" % name)
        # No other import may start inside the bytes this one's name holds (a shared tail).
        inside = [s for s in starts if offset < s <= ends[offset]]
        if inside:
            raise ValueError("%s shares its bytes with another name" % name)
        data[pool + offset:pool + offset + len(new)] = new
        renamed.add(offset)
        done += 1
    with open(path, "wb") as f:
        f.write(data)
    return done


if __name__ == "__main__":
    with open(sys.argv[3], encoding="utf-8") as f:
        pairs = dict(line.split() for line in f if line.strip())
    print("%d imports renamed" % rename_imports(sys.argv[1], sys.argv[2], pairs))
