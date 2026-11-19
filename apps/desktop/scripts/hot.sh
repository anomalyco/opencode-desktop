#!/usr/bin/env bash
# A hot-editable OpenCode (Rune) window on the live server (http://127.0.0.1:4096 unless
# --server). Watches the desktop view tree (`src/shell/view/**/*.rs`); any edit outside it
# (including `crates/opencode-ui/src/palette.rs`, `features/*`, and other crates) rebuilds and
# restarts the app, which reopens the same tabs, session and drafts.
#   apps/desktop/scripts/hot.sh [APP ARGS]     e.g. --read-only, --server URL, --fixture FILE
# Headless proof instead of a window: --snap-on-patch /abs/out.png [--hot-control /abs/c.json]
# Note: `scripts/rune-hot` only matches `--patch` globs inside `apps/desktop/`, and Subsecond
# 0.7.10 only live-patches a tip binary crate (`apps/desktop` now builds a `[lib]` plus thin
# `[[bin]]` wrapper). For fast UI iteration today, prefer Luau `--plugin-hot` or `scripts/dev-fast.sh`.
# The machinery is the repo's scripts/rune-hot.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ARGS=("$@")
[[ " $* " == *" --live "* || " $* " == *" --fixture "* ]] || ARGS=(--live "$@")
exec "$HERE/../../../scripts/rune-hot" --manifest "$HERE/../Cargo.toml" --patch 'src/shell/view/**/*.rs' \
  --port "${HOT_PORT:-18745}" -- "${ARGS[@]}"
