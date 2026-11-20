#!/usr/bin/env bash
# Checks that dialogs take one of Desktop's three size presets (`opencode_ui::dialog::DialogSize`,
# re-exported as `session_view::dialog::DialogSize`: normal 480, large 680 × 600, x-large 880 × 680)
# instead of sizing themselves. A line that really needs a literal ends in `// dialog-size: ok`.
# Usage: scripts/dialog-size.sh
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"
exec bash "$ROOT/scripts/guardrails/run-js.sh" "$ROOT/scripts/guardrails/dialog-size.mjs" "$@"
