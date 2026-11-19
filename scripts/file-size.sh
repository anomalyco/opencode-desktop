#!/usr/bin/env bash
# Repo-wide Rust file-size monitor and 1,500-line exception ratchet across every tracked .rs file
# under apps/, crates/, and features/ — including files outside scripts/arch-check.sh Rule 8
# (apps/web/, apps/installer/, apps/browser-engine/, crates/browser-pane/, and generated files).
#   warn  > 800 lines
#   error > 1500 lines unless listed in scripts/file-size-exceptions.txt ("<path> <ceiling>")
#
# Distinct roles:
#   - scripts/file-size.sh   repo-wide 800-warn / 1500-error ceiling with downward-only exceptions
#   - scripts/arch-check.sh  Rule 8 enforces the stricter <= 800 hard cap (no exceptions) on
#                            apps/desktop/, features/, and hand-written workspace crates/
#
# Usage: scripts/file-size.sh [--warn-only] [--update-exceptions [--allow-raise]]
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

SOFT=800 HARD=1500
EXC=scripts/file-size-exceptions.txt
warn_only=0 update=0 raise=0
for a in "$@"; do
  case "$a" in
    --warn-only) warn_only=1 ;;
    --update-exceptions) update=1 ;;
    --allow-raise) raise=1 ;;
    *) echo "unknown option: $a" >&2; exit 2 ;;
  esac
done

measure() {
  git ls-files -z 'apps/*.rs' 'crates/*.rs' 'features/*.rs' | xargs -0 awk '
    FNR == 1 { if (name != "") printf "%s %d\n", name, lines; name = FILENAME; lines = 0 }
    { lines++ }
    END { if (name != "") printf "%s %d\n", name, lines }'
}

rows=$(measure)
ceil_of() { [ -f "$EXC" ] && awk -v p="$1" '$1 == p { print $2; exit }' "$EXC" || true; }

errors=0 warns=0 oversoft=0 overhard=0
out=$(mktemp); trap 'rm -f "$out"' EXIT
ann() { [ "${GITHUB_ACTIONS:-}" = true ] && echo "::$1 file=$2::$3" || true; }

while read -r p lines; do
  [ -n "${p:-}" ] || continue
  [ "$lines" -gt "$SOFT" ] || continue
  oversoft=$((oversoft + 1))
  if [ "$lines" -gt "$HARD" ]; then
    overhard=$((overhard + 1))
    limit=$(ceil_of "$p"); limit=${limit:-$HARD}
    if [ "$lines" -gt "$limit" ]; then
      errors=$((errors + 1))
      printf '%6d  %-6s %s (limit %d)\n' "$lines" "ERROR" "$p" "$limit" >> "$out"
      ann error "$p" "$lines lines, limit $limit; split a module out before adding more"
    else
      warns=$((warns + 1))
      printf '%6d  %-6s %s (exception, ceiling %d)\n' "$lines" "warn" "$p" "$limit" >> "$out"
      ann warning "$p" "$lines lines (exception ceiling $limit, soft limit $SOFT)"
    fi
  else
    warns=$((warns + 1))
    printf '%6d  %-6s %s\n' "$lines" "warn" "$p" >> "$out"
    ann warning "$p" "$lines lines (soft limit $SOFT)"
  fi
done <<< "$rows"

sort -rn "$out"
echo "over $SOFT: $oversoft, over $HARD: $overhard, errors: $errors, warnings: $warns"

if [ "$update" -eq 1 ]; then
  tmp=$(mktemp)
  {
    echo "# <path> <ceiling>: line ceilings for files above $HARD. Ceilings only go down."
    echo "# Refresh with scripts/file-size.sh --update-exceptions."
    while read -r p lines; do
      [ -n "${p:-}" ] || continue
      [ "$lines" -gt "$HARD" ] || continue
      old=$(ceil_of "$p")
      if [ -z "$old" ]; then
        if [ "$raise" -eq 1 ]; then echo "$p $lines"; else echo "not listing new oversized file $p ($lines); use --allow-raise" >&2; fi
      elif [ "$lines" -gt "$old" ] && [ "$raise" -eq 0 ]; then
        echo "$p $old"
      else
        echo "$p $lines"
      fi
    done <<< "$rows" | sort
  } > "$tmp"
  mv "$tmp" "$EXC"
  echo "wrote $EXC ($(grep -vc '^#' "$EXC") entries)"
  exit 0
fi

[ "$warn_only" -eq 1 ] && exit 0
[ "$errors" -eq 0 ]
