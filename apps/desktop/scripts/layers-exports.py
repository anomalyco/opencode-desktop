#!/usr/bin/env python3
"""Links each code library of a layered macOS build again so that it exports only the names another file takes.

    layers-exports.py <folder of the built exe> <exe>

build-app.sh runs it after cargo when OPENCODE_RUNE_LAYERS=1. Why: at every launch dyld walks every
exported name of every library it loads (mach_o::GenericTrie::validate) and looks up each name one
file takes from another (GenericTrie::find); measured on the Air, 51,700 exported names and 12,600
taken ones cost 15 ms before `main`. A Rust `dylib` exports every public function of every crate in
it, because rustc cannot know who will link it. Here everything is built, so the set is known: for
each library, highest first, the names the exe and the libraries above it take from it. With only
those exported the linker also drops the code nothing reaches.

link-order.sh kept each library's link (its arguments and the objects rustc deletes afterwards) in
<library>.relink/. The exe is not linked again: the names it takes are all still there.
"""
import os
import re
import subprocess
import sys

TAKES = re.compile(r"\(undefined\) (?:weak )?external (\S+) \(from (libopencode_layer_[a-z0-9]+)\)")


def fail(text):
    sys.exit("layers-exports.py: " + text)


def run(*args, **kwargs):
    done = subprocess.run(args, stdout=subprocess.PIPE, stderr=subprocess.PIPE, universal_newlines=True, **kwargs)
    if done.returncode:
        fail("%s failed:\n%s" % (" ".join(args[:3]), done.stderr[-3000:]))
    return done.stdout


def takes(path):
    """The names a file takes from each code library."""
    out = {}
    for name, library in TAKES.findall(run("nm", "-m", "-u", path)):
        out.setdefault(library, set()).add(name)
    return out


def exported(path):
    return set(run("nm", "-g", "-U", "-j", path).split())


def main():
    folder, exe = sys.argv[1], sys.argv[2]
    gen = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "layers", "gen")
    with open(os.path.join(gen, "names.txt"), encoding="utf-8") as f:
        names = f.read().split()
    above = [exe]
    for name in reversed(names):
        library = "libopencode_layer_%s" % name
        built = os.path.join(folder, "deps", library + ".dylib")
        kept = built + ".relink"
        if not os.path.isfile(os.path.join(kept, "args")):
            fail("no kept link for %s (link-order.sh writes %s)" % (library, kept))
        wanted = set()
        for path in above:
            wanted |= takes(path).get(library, set())
        before = (os.path.getsize(built), len(exported(built)))
        if not wanted:
            fail("nothing takes a name from %s" % library)
        listed = os.path.join(kept, "exports")
        with open(listed, "w", encoding="utf-8", newline="\n") as f:
            f.write("\n".join(sorted(wanted)) + "\n")
        with open(os.path.join(kept, "args"), encoding="utf-8") as f:
            args = f.read().split("\n")[:-1]
        with open(os.path.join(kept, "cwd"), encoding="utf-8") as f:
            cwd = f.read().strip()
        # rustc's two helper objects: one refers to every name of its own export list (it would keep
        # all the code), the other is the crate metadata the compiler reads, which no launch needs.
        args = [a for a in args if not a.endswith(("-symbols.o", "-rmeta.o"))]
        run("cc", *args, "-Wl,-exported_symbols_list," + listed, "-Wl,-dead_strip", cwd=cwd)
        missing = wanted - exported(built)
        if missing:
            fail("%s lost %d names another file takes, for example %s" % (library, len(missing), sorted(missing)[0]))
        # Cargo's copy beside the exe is a hard link to the file the linker has just replaced.
        beside = os.path.join(folder, library + ".dylib")
        if os.path.exists(beside):
            os.remove(beside)
        run("cp", built, beside)
        print("layers-exports.py: %s: %d -> %d bytes, %d -> %d exported names" % (library, before[0], os.path.getsize(built), before[1], len(wanted)), file=sys.stderr)
        above.append(built)


main()
