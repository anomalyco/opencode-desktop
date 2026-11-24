#!/usr/bin/env python3
"""Writes the crates that make the macOS app's code several libraries instead of one exe.

    layers.py <target triple> [--meta <cargo metadata json>] [--check]

build-app.sh runs it when OPENCODE_RUNE_LAYERS=1. It reads apps/desktop/layers/layers.txt (one
library per line, lowest first) and the build's crate graph (`cargo metadata`), and writes, under
apps/desktop/layers/gen/ (never checked in):

  <name>/Cargo.toml, src/lib.rs   a `dylib` crate per line: it names the line's crates, so the
                                  library holds them and all they need that no lower library holds
  names.txt                       the libraries, lowest first (layers-exports.py)
  all.rs                          `extern crate` for each, which `main.rs` includes (cfg code_layers)

then names them as dependencies at the end of apps/desktop/Cargo.toml and lets cargo add them to
Cargo.lock (nothing else in the lock may change: checked). The crates are generated from the lock
on every build, so a dependency bump never needs an edit here.

--check prints the libraries and their crates and writes nothing.
"""
import json
import os
import re
import subprocess
import sys

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", ".."))
LAYERS = os.path.join(ROOT, "apps", "desktop", "layers")
GEN = os.path.join(LAYERS, "gen")
MARK = "# code layers: written by apps/desktop/scripts/layers.py for this build; never checked in\n"


def fail(text):
    sys.exit("layers.py: " + text)


def cargo(*args):
    done = subprocess.run(["cargo", *args], cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE, universal_newlines=True)
    if done.returncode:
        fail("cargo %s failed:\n%s" % (" ".join(args), done.stderr[-2000:]))
    return done.stdout


def main():
    args = sys.argv[1:]
    check = "--check" in args
    meta_file = args[args.index("--meta") + 1] if "--meta" in args else None
    triple = args[0] if args and not args[0].startswith("--") else fail("no target triple")
    if meta_file:
        with open(meta_file, encoding="utf-8") as f:
            meta = json.load(f)
    else:
        meta = json.loads(cargo("metadata", "--format-version", "1", "--filter-platform", triple, "--locked"))
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = os.path.normpath(meta["workspace_root"])

    def macro(i):
        return any("proc-macro" in t["kind"] for t in packages[i]["targets"])

    def needs(i):
        # What a crate links: its normal dependencies. A macro's own dependencies run in the compiler only.
        if macro(i):
            return []
        return [d["pkg"] for d in nodes[i]["deps"] if any(k["kind"] is None for k in d["dep_kinds"]) and not macro(d["pkg"])]

    def closure(ids, stop=()):
        out, todo = set(), list(ids)
        while todo:
            i = todo.pop()
            if i not in out and i not in stop:
                out.add(i)
                todo.extend(needs(i))
        return out

    def under(i, *parts):
        return os.path.normpath(os.path.dirname(packages[i]["manifest_path"])).startswith(os.path.join(root, *parts) + os.sep)

    def family(i):
        source = packages[i]["source"] or ""
        if not source:
            return "third" if under(i, "vendor") else ("feature" if under(i, "features") else "app")
        if re.search(r"anomalyco/rune([?#]|$)", source):
            return "rune"
        if re.search(r"betteroffice", source, re.I):
            return "office"
        return "third"

    app = next(i for i, p in packages.items() if p["name"] == "opencode-desktop")
    graph = closure([app])
    engines = {i for i in graph if family(i) == "office"}
    # What only the office engines need: not reached from the app when the engines are left out.
    office_only = closure(engines) - closure([app], stop=engines)
    tokens = {
        "@rune": [i for i in graph if family(i) == "rune"],
        "@office": [i for i in graph if family(i) == "office"],
        "@third": [i for i in graph if family(i) == "third" and i not in office_only],
        "@app": [i for i in graph if family(i) == "app" and i != app],
        "@features": [i for i in graph if family(i) == "feature"],
    }
    by_name = {}
    for i in graph:
        by_name.setdefault(packages[i]["name"], []).append(i)

    with open(os.path.join(LAYERS, "layers.txt"), encoding="utf-8") as f:
        lines = [line.split("#")[0].strip().partition(":") for line in f]
    # `exe:` names crates that stay in the exe, with what only they need.
    kept = {app} | {i for name, _, names in lines if name.strip() == "exe" for word in names.split() for i in by_name.get(word, [])}
    layers, placed = [], set()
    for name, _, names in lines:
        name = name.strip()
        if not name or name == "exe":
            continue
        if not re.fullmatch(r"[a-z][a-z0-9]*", name):
            fail("a library's name is lower-case letters and digits: %r" % name)
        wanted, typed = [], set()
        for word in names.split():
            found = tokens[word] if word in tokens else by_name.get(word)
            if found is None:
                # A crate this build does not have (another platform's, a feature that is off) is nothing to place.
                print("layers.py: %s: no crate named %s in this build" % (name, word), file=sys.stderr)
                continue
            wanted.extend(i for i in found if i not in kept)
            if word not in tokens:
                typed.update(found)
        inside = closure(wanted) - placed
        if inside & kept:
            fail("%s would hold %s, which must stay in the exe" % (name, ", ".join(sorted(packages[i]["name"] for i in inside & kept))))
        # The crates to name: the ones the line spells out, and any other that nothing else in the
        # library needs. Not every crate of a family: `cargo metadata` also lists crates that this
        # build never compiles (behind a feature another platform turns on), and naming one of
        # those would build it, with no features.
        needed = {d for i in inside for d in needs(i)}
        named = sorted({i for i in typed if i in inside} | {i for i in inside if i not in needed}, key=lambda i: (packages[i]["name"], packages[i]["version"]))
        if inside:
            layers.append({"name": name, "inside": inside, "named": named})
            placed |= inside
    if not layers or layers[0]["name"] != "base":
        fail("the first library must be `base` (the standard library and the allocator)")
    for want in ("procwatch", "mimalloc"):
        if not any(packages[i]["name"] == want for i in layers[0]["named"]):
            fail("`base` must name %s (layers/base.rs)" % want)

    for layer in layers:
        print("layers.py: %-7s %3d crates: %s" % (layer["name"], len(layer["inside"]), " ".join(sorted(packages[i]["name"] for i in layer["inside"]))[:600]), file=sys.stderr)
    print("layers.py: exe     %3d crates: %s" % (len(graph - placed), " ".join(sorted(packages[i]["name"] for i in graph - placed))[:600]), file=sys.stderr)
    if check:
        return

    def spec(i, key):
        p = packages[i]
        source = p["source"] or ""
        out = ['package = "%s"' % p["name"]]
        if not source and not under(i, "vendor"):
            out.append('path = "%s"' % os.path.relpath(os.path.dirname(p["manifest_path"]), os.path.join(GEN, "x")).replace(os.sep, "/"))
        elif source.startswith("git+"):
            url, _, rest = source[4:].partition("#")
            url, _, query = url.partition("?")
            out.append('git = "%s"' % url)
            for part in filter(None, query.split("&")):
                k, _, v = part.partition("=")
                if k in ("rev", "branch", "tag"):
                    out.append('%s = "%s"' % (k, v))
        else:
            # The registry's crate, or the copy under vendor/ that the workspace patches in for it.
            out.append('version = "=%s"' % p["version"])
        out.append("default-features = false")
        return "%s = { %s }" % (key, ", ".join(out))

    if os.path.isdir(GEN):
        import shutil

        shutil.rmtree(GEN)
    with open(os.path.join(LAYERS, "base.rs"), encoding="utf-8") as f:
        base = f.read()
    previous = None
    for layer in layers:
        folder = os.path.join(GEN, layer["name"])
        os.makedirs(os.path.join(folder, "src"))
        keys, lines, source = {}, [], []
        counts = {}
        for i in layer["named"]:
            counts[packages[i]["name"]] = counts.get(packages[i]["name"], 0) + 1
        for n, i in enumerate(layer["named"]):
            name = packages[i]["name"]
            keys[i] = name if counts[name] == 1 else "%s-v%d" % (name, n)
            lines.append(spec(i, keys[i]))
            source.append("extern crate %s;" % keys[i].replace("-", "_"))
        if previous:
            lines.insert(0, 'opencode-layer-%s = { path = "../%s" }' % (previous, previous))
            source.insert(0, "extern crate opencode_layer_%s;" % previous)
        with open(os.path.join(folder, "Cargo.toml"), "w", encoding="utf-8", newline="\n") as f:
            f.write(MARK + '[package]\nname = "opencode-layer-%s"\nversion = "0.0.0"\nedition = "2021"\npublish = false\n\n[lib]\ncrate-type = ["dylib"]\ntest = false\ndoctest = false\n\n[dependencies]\n%s\n' % (layer["name"], "\n".join(lines)))
        with open(os.path.join(folder, "src", "lib.rs"), "w", encoding="utf-8", newline="\n") as f:
            f.write("// " + MARK[2:] + "#![allow(unused_extern_crates)]\n" + "\n".join(source) + "\n" + (base if layer["name"] == "base" else ""))
        previous = layer["name"]
    with open(os.path.join(GEN, "names.txt"), "w", encoding="utf-8", newline="\n") as f:
        f.write("".join(layer["name"] + "\n" for layer in layers))
    with open(os.path.join(GEN, "all.rs"), "w", encoding="utf-8", newline="\n") as f:
        f.write("".join("extern crate opencode_layer_%s;\n" % layer["name"] for layer in layers))

    manifest = os.path.join(ROOT, "apps", "desktop", "Cargo.toml")
    with open(manifest, encoding="utf-8") as f:
        text = f.read().split(MARK)[0].rstrip("\n") + "\n"
    with open(manifest, "w", encoding="utf-8", newline="\n") as f:
        f.write(text + "\n" + MARK + "".join('[dependencies.opencode-layer-%s]\npath = "layers/gen/%s"\n' % (layer["name"], layer["name"]) for layer in layers))

    # Cargo adds the new crates to the lock; every crate they name is already there, at the same version.
    lock = os.path.join(ROOT, "Cargo.lock")
    with open(lock, encoding="utf-8") as f:
        before = f.read()
    cargo("metadata", "--format-version", "1", "--filter-platform", triple, "--offline")
    with open(lock, encoding="utf-8") as f:
        after = f.read()

    def entries(text):
        return {e.strip() for e in text.split("[[package]]") if e.strip()}

    def without_layers(entry):
        return "\n".join(line for line in entry.split("\n") if "opencode-layer-" not in line)

    old, new = entries(before), entries(after)
    lost = {without_layers(e) for e in old} - {without_layers(e) for e in new}
    extra = [e for e in new - old if 'name = "opencode-layer-' not in e and without_layers(e) not in {without_layers(o) for o in old}]
    if lost or extra:
        fail("Cargo.lock changed beyond the layer crates:\n%s" % "\n---\n".join(sorted(lost | set(extra)))[:3000])
    print("layers.py: %d libraries: %s" % (len(layers), " ".join(layer["name"] for layer in layers)), file=sys.stderr)


main()
