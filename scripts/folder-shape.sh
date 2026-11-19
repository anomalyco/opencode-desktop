#!/usr/bin/env bash
# Checks folder shape for tracked sources.
#   prefix family  x.rs or x/ plus 2+ x_*.rs siblings, or 3+ x_*.rs alone: error, "move into x/"
#   folder breadth > 40 non-test source files in one folder: warn; > 60: error
# Existing violations are listed in scripts/folder-shape-exceptions.txt; ceilings only go down.
# Usage: scripts/folder-shape.sh [--warn-only] [--update-exceptions [--allow-raise]]
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"
exec bash "$ROOT/scripts/guardrails/run-js.sh" "$ROOT/scripts/guardrails/folder-shape.mjs" "$@"
