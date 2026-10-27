#!/usr/bin/env bash
# A hot-editable OpenCode (Rune) window on the live server (http://127.0.0.1:4096 unless
# --server). Saved edits to function bodies in src/view.rs and src/palette.rs patch into the
# open window; any other edit restarts the app, which reopens the same tabs, session and drafts.
#   apps/desktop/scripts/hot.sh [APP ARGS]     e.g. --read-only, --server URL, --fixture FILE
# Headless proof instead of a window: --snap-on-patch /abs/out.png [--hot-control /abs/c.json]
# The machinery is the repo's scripts/rune-hot.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ARGS=("$@")
[[ " $* " == *" --live "* || " $* " == *" --fixture "* ]] || ARGS=(--live "$@")
exec "$HERE/../../../scripts/rune-hot" --manifest "$HERE/../Cargo.toml" --patch src/view.rs --patch src/palette.rs \
  --port "${HOT_PORT:-18745}" -- "${ARGS[@]}"
