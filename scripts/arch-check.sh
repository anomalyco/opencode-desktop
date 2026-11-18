#!/usr/bin/env bash
# Ironclad workspace architecture guardrails for rune-desktop.
# Enforces progressive disclosure, acyclic wide-shallow crate graph, and App field ratchet:
#   1. Strict downward crate dependency layers: apps/* -> features/* -> crates/* (zero cycles)
#   2. Zero cross-feature crate dependencies under features/* (communicate via opencode-kernel / Luau Contracts)
#   3. crates/opencode-kernel, crates/opencode-runtime, crates/opencode-ui, and crates/opencode-search forbid unsafe_code
#   4. apps/desktop/src/ root stays clean (<= 10 top-level .rs files; domains live in subfolders)
#   5. struct App field-count ratchet (ceilings only go down; new features use opencode-kernel StateSlots or Luau)
#   6. apps/web/Cargo.toml depends on opencode-desktop (path = "../desktop") and never duplicates features/* deps
#   7. No #[test] or #[cfg(test)] in tracked workspace sources (AGENTS.md)
#   8. Hard <= 800 non-test line cap on every tracked .rs file in apps/desktop/ and features/
#   9. Pure feature crates under features/*/src/lib.rs forbid unsafe_code (outside explicit FFI/GPU allowlist)
# Usage: scripts/arch-check.sh
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

exec python3 - <<'PY'
import glob, os, re, subprocess, sys
from collections import defaultdict

# Ratchet ceiling for fields on `pub struct App` in apps/desktop/src/app.rs.
# Only lower this number; new features must use opencode-kernel StateSlots or Luau plugins.
APP_FIELD_CEILING = 142
DESKTOP_ROOT_MAX_FILES = 10
HARD_LINE_CAP = 800
FEATURE_UNSAFE_ALLOWLIST = {"egg", "office", "open-in", "updater"}

errors = []

def read_text(path):
    with open(path, "r", encoding="utf-8", errors="ignore") as f:
        return f.read()

def parse_pkg_name(toml_path):
    text = read_text(toml_path)
    m = re.search(r'(?m)^name\s*=\s*"([^"]+)"', text)
    return m.group(1) if m else os.path.basename(os.path.dirname(toml_path))

def parse_path_deps(toml_path):
    base = os.path.dirname(toml_path)
    text = read_text(toml_path)
    deps = []
    for m in re.finditer(r'path\s*=\s*"([^"]+)"', text):
        rel = m.group(1)
        target = os.path.normpath(os.path.join(base, rel)).replace("\\", "/")
        deps.append((rel, target))
    return deps

# Collect all workspace crates by tier
manifests = (
    [(p, "crate", 1) for p in sorted(glob.glob("crates/*/Cargo.toml"))] +
    [(p, "feature", 2) for p in sorted(glob.glob("features/*/Cargo.toml"))] +
    [(p, "app", 3) for p in sorted(glob.glob("apps/*/Cargo.toml")) if "browser-engine" not in p]
)

dir_to_meta = {}
graph = defaultdict(list)

for toml, tier, rank in manifests:
    d = os.path.dirname(toml).replace("\\", "/")
    dir_to_meta[d] = (parse_pkg_name(toml), tier, rank, toml)

for toml, tier, rank in manifests:
    src_dir = os.path.dirname(toml).replace("\\", "/")
    src_name = dir_to_meta[src_dir][0]
    for rel, dst_dir in parse_path_deps(toml):
        if dst_dir not in dir_to_meta:
            continue
        dst_name, dst_tier, dst_rank, _ = dir_to_meta[dst_dir]
        graph[src_dir].append(dst_dir)

        # Rule 1: Strict downward layer dependencies
        if dst_rank > rank:
            errors.append(f"{toml}: upward dependency {src_name} ({tier}) -> {dst_name} ({dst_tier})")

        # Rule 2: Zero cross-feature crate dependencies
        if tier == "feature" and dst_tier == "feature" and dst_dir != src_dir:
            errors.append(f"{toml}: cross-feature dependency {src_name} -> {dst_name}; decouple via opencode-kernel")

        # Rule 2b: opencode-kernel, opencode-runtime, and opencode-ui must stay minimal
        if src_name == "opencode-kernel" and dst_name in {"session-view", "terminal", "browser-pane", "office-preview", "opencode-storage"}:
            errors.append(f"{toml}: opencode-kernel must stay minimal and not depend on {dst_name}")
        if src_name == "opencode-runtime" and (
            dst_tier in {"feature", "app"}
            or dst_name in {"session-view", "terminal", "browser-pane", "office-preview", "opencode-storage"}
        ):
            errors.append(f"{toml}: opencode-runtime must stay headless and not depend on {dst_name}")
        if src_name == "opencode-ui" and (
            dst_tier in {"feature", "app"}
            or dst_name in {"session-view", "opencode-storage", "terminal", "browser-pane", "office-preview", "opencode-runtime"}
        ):
            errors.append(f"{toml}: opencode-ui must stay pure UI foundation and not depend on {dst_name}")
        if src_name == "opencode-code-view" and (
            dst_tier in {"feature", "app"}
            or dst_name in {"session-view", "opencode-storage", "terminal", "browser-pane", "office-preview", "opencode-runtime"}
        ):
            errors.append(f"{toml}: opencode-code-view must not depend on {dst_name}")
        if src_name == "opencode-search" and (
            dst_tier in {"feature", "app"}
            or dst_name in {"session-view", "opencode-ui", "opencode-code-view", "opencode-storage", "terminal", "browser-pane", "office-preview", "opencode-runtime"}
        ):
            errors.append(f"{toml}: opencode-search must stay strictly headless and not depend on {dst_name}")
        if src_name == "opencode-plugin-runtime" and (
            dst_tier in {"feature", "app"}
            or dst_name in {"session-view", "opencode-storage", "terminal", "browser-pane", "office-preview"}
        ):
            errors.append(f"{toml}: opencode-plugin-runtime must stay App-independent and not depend on {dst_name}")

    if src_name == "opencode-search" and re.search(r'(?m)^\s*rune(?:-[a-z0-9_-]+)?\s*=', read_text(toml)):
        errors.append(f"{toml}: opencode-search must stay strictly free of rune/UI dependencies")

# Rule 1b: Full workspace DFS cycle check
WHITE, GRAY, BLACK = 0, 1, 2
color = {n: WHITE for n in dir_to_meta}
stack = []

def dfs(u):
    color[u] = GRAY
    stack.append(u)
    for v in graph[u]:
        if color.get(v) == GRAY:
            cycle = " -> ".join(stack[stack.index(v):] + [v])
            errors.append(f"circular workspace crate dependency: {cycle}")
        elif color.get(v) == WHITE:
            dfs(v)
    stack.pop()
    color[u] = BLACK

for node in sorted(dir_to_meta):
    if color[node] == WHITE:
        dfs(node)

# Rule 3: opencode-kernel, opencode-runtime, opencode-ui, and opencode-search forbid unsafe_code
for core_lib in (
    "crates/opencode-kernel/src/lib.rs",
    "crates/opencode-runtime/src/lib.rs",
    "crates/opencode-ui/src/lib.rs",
    "crates/opencode-search/src/lib.rs",
):
    if os.path.exists(core_lib):
        if "#![forbid(unsafe_code)]" not in read_text(core_lib):
            errors.append(f"{core_lib}: must declare #![forbid(unsafe_code)]")
    else:
        errors.append(f"{core_lib}: missing core crate")

# Rule 4: apps/desktop/src root breadth
desktop_root_rs = [p for p in glob.glob("apps/desktop/src/*.rs") if os.path.isfile(p)]
if len(desktop_root_rs) > DESKTOP_ROOT_MAX_FILES:
    errors.append(
        f"apps/desktop/src/: {len(desktop_root_rs)} root .rs files (max {DESKTOP_ROOT_MAX_FILES}); "
        "group into domain folders (shell, session, workspace, surfaces, dialogs, stories)"
    )

# Rule 5: struct App field-count ratchet
app_rs = "apps/desktop/src/app.rs"
if os.path.exists(app_rs):
    text = read_text(app_rs)
    m = re.search(r"pub struct App\s*\{(.*?)\n\}", text, re.DOTALL)
    if m:
        body = m.group(1)
        fields = [
            line for line in body.splitlines()
            if re.match(r"^\s*(?:pub(?:\([^)]*\))?\s+)?[a-z_][a-z0-9_]*\s*:", line)
        ]
        if len(fields) > APP_FIELD_CEILING:
            errors.append(
                f"{app_rs}: struct App has {len(fields)} fields (ceiling {APP_FIELD_CEILING}); "
                "store new feature state in opencode_kernel::StateSlots or build a Luau plugin"
            )
else:
    errors.append(f"{app_rs}: missing App definition for field-count ratchet")

# Rule 6: apps/web depends on opencode-desktop's library target and never duplicates features/* deps
web_toml = "apps/web/Cargo.toml"
if os.path.exists(web_toml):
    web_path_deps = [d for _, d in parse_path_deps(web_toml) if d in dir_to_meta]
    if "apps/desktop" not in web_path_deps:
        errors.append(f'{web_toml}: must depend on opencode-desktop via path = "../desktop"')
    dup_features = sorted(
        dir_to_meta[d][0] for d in web_path_deps if dir_to_meta[d][1] == "feature"
    )
    if dup_features:
        errors.append(
            f"{web_toml}: duplicates features/* dependencies ({', '.join(dup_features)}); "
            "depend on opencode-desktop instead"
        )

# Rule 7: Zero #[test] or #[cfg(test)] in tracked Rust sources
files = [
    f for f in subprocess.check_output(["git", "ls-files", "apps/*.rs", "crates/*.rs", "features/*.rs"], text=True).splitlines()
    if f and not f.startswith("vendor/")
]
test_attr = re.compile(r"^\s*#\[(?:cfg\(test\)|test)\b", re.MULTILINE)
for f in files:
    if os.path.exists(f) and test_attr.search(read_text(f)):
        errors.append(f"{f}: contains #[test] or #[cfg(test)] (forbidden by AGENTS.md)")

# Rule 8: Hard <= 800 line ceiling on every tracked .rs file in apps/desktop/ and features/
for f in files:
    if not (f.startswith("apps/desktop/") or f.startswith("features/")):
        continue
    if os.path.exists(f):
        line_count = len(read_text(f).splitlines())
        if line_count > HARD_LINE_CAP:
            errors.append(f"{f}: {line_count} lines exceeds hard ceiling of {HARD_LINE_CAP} in apps/desktop/ and features/")

# Rule 9: Pure feature crates under features/*/src/lib.rs forbid unsafe_code
for lib in sorted(glob.glob("features/*/src/lib.rs")):
    feat = os.path.basename(os.path.dirname(os.path.dirname(lib)))
    if feat in FEATURE_UNSAFE_ALLOWLIST:
        continue
    if "#![forbid(unsafe_code)]" not in read_text(lib):
        errors.append(f"{lib}: pure feature crate must declare #![forbid(unsafe_code)]")

if errors:
    for e in errors:
        print(f"  ERROR  {e}")
    print(f"arch check: {len(errors)} error(s)")
    sys.exit(1)

print(
    f"arch check: ok "
    f"({sum(1 for _, t, _ in manifests if t == 'feature')} features, "
    f"{sum(1 for _, t, _ in manifests if t == 'crate')} crates, "
    f"{len(desktop_root_rs)} apps/desktop/src root files, 0 cycles, 0 files >{HARD_LINE_CAP} in desktop/features)"
)
PY
