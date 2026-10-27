#!/usr/bin/env bash
# A hot-editable OpenCode (Rune) window on the live server (http://127.0.0.1:4096 unless
# --server). Saved edits to function bodies in src/view.rs and src/palette.rs patch into the
# open window; any other edit restarts the app, which reopens the same tabs and zoom.
#   apps/desktop/scripts/hot.sh [APP ARGS]     e.g. --read-only, --server URL, --fixture FILE
# Headless proof instead of a window: --snap-on-patch /abs/out.png [--hot-control /abs/c.json]
#
# Needs Dioxus CLI 0.7.10 at ~/.cargo/bin/dx (RUNE_DX overrides) and a Rune checkout for
# scripts/rune-dev and its edit guard (RUNE, default ~/code/open-source/rune). The compiler and
# the app run together in one HiveMind cargo lease on this Mac.
set -euo pipefail
SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
cd "$(dirname "$SELF")/.."
RUNE=${RUNE:-$HOME/code/open-source/rune}
test -x "$RUNE/scripts/rune-dev" || { echo "hot.sh: no scripts/rune-dev in $RUNE (set RUNE)" >&2; exit 1; }

if [[ -z "${HIVEMIND_ACTIVE:-}" ]]; then
  exec hivemind run --kind cargo --worker "${HOT_WORKER:-studio}" --no-sync -- "$SELF" "$@"
fi

exec python3 - "$RUNE/scripts/rune-dev" "$@" <<'PY'
import os, signal, subprocess, sys

rune_dev, args = sys.argv[1], sys.argv[2:]
if not any(a in args for a in ("--live", "--fixture")):
    args = ["--live", *args]
# rune-dev assumes the compiler runs on a separate build Mac and refuses to run on Studio.
# Run its in-lease path (dx, the edit guard and the app) right here instead.
runner = ("import runpy, socket, sys; socket.gethostname = lambda: 'local'; "
          "sys.argv = sys.argv[1:]; runpy.run_path(sys.argv[0], run_name='__main__')")
command = [sys.executable, "-c", runner, rune_dev, "--manifest-path", "Cargo.toml",
           "--patch", "src/view.rs", "--patch", "src/palette.rs", "--port", os.environ.get("HOT_PORT", "18745"), "--", *args]
env = dict(os.environ, RUNE_DEV_LEASE="1")
toolchain = os.path.expanduser("~/.rustup/toolchains/stable-aarch64-apple-darwin/bin")
if os.path.isdir(toolchain):
    env["PATH"] = toolchain + os.pathsep + env["PATH"]

while True:
    child = subprocess.Popen(command, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1)
    restart = False
    try:
        for line in child.stdout:
            print(line, end="", flush=True)
            if not restart and "rune-dev: restart needed" in line:
                restart = True
                child.send_signal(signal.SIGTERM)
        child.wait()
    except KeyboardInterrupt:
        child.send_signal(signal.SIGTERM)
        child.wait()
        sys.exit(130)
    if not restart:
        sys.exit(child.returncode)
    print("hot.sh: restarting with the new build", flush=True)
PY
