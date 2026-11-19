#!/usr/bin/env bash
# Repo-wide Rust file-size monitor and 1,500-line exception ratchet across every tracked .rs file
# under apps/, crates/, and features/ — including files outside scripts/arch-check.sh Rule 8
# (apps/web/, apps/installer/, apps/browser-engine/, crates/browser-pane/, and generated files).
#   warn  > 800 lines
#   error > 1500 lines unless listed in scripts/file-size-exceptions.txt ("<path> <ceiling>")
#
# Usage: scripts/file-size.sh [--warn-only] [--update-exceptions [--allow-raise]]
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"
exec bash "$ROOT/scripts/guardrails/run-js.sh" "$ROOT/scripts/guardrails/file-size.mjs" "$@"
