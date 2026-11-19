#!/usr/bin/env bash
# Runs a plain-ESM guardrail script (.mjs) with bun (preferred) or node >= 18.
# Fails loudly (exit 127) if neither working runtime is installed.
set -euo pipefail

if [ "$#" -lt 1 ]; then
  echo "usage: run-js.sh <script.mjs> [args...]" >&2
  exit 2
fi

script="$1"
shift

if command -v bun >/dev/null 2>&1 && bun --version >/dev/null 2>&1; then
  exec bun "$script" "$@"
fi

if command -v node >/dev/null 2>&1 && node_ver="$(node --version 2>/dev/null)"; then
  major="${node_ver#v}"
  major="${major%%.*}"
  if [[ "$major" =~ ^[0-9]+$ ]] && [ "$major" -ge 18 ]; then
    exec node "$script" "$@"
  fi
  echo "ERROR: guardrails require bun or node >= 18 (found node $node_ver)" >&2
  exit 127
fi

echo "ERROR: guardrails require bun or node >= 18 on PATH (neither working runtime was found)" >&2
exit 127
