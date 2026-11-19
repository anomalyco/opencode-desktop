#!/usr/bin/env bash
# Checks hand-maintained tables and generated contract bindings:
#   - apps/desktop/src/stories/order.rs <-> STORY_LIST registrations
#   - apps/desktop/src/shell/commands/order.rs <-> built-in CommandEntry registrations
#   - crates/opencode-i18n/locales/rune/*.json coverage & unknown keys vs en.json
#   - crates/opencode-plugin-spec generated files <-> contract.json (`--check`)
# Usage: scripts/tables-check.sh
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"
exec bash "$ROOT/scripts/guardrails/run-js.sh" "$ROOT/scripts/guardrails/tables-check.mjs" "$@"
