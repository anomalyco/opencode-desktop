#!/usr/bin/env bash
# Checks that dialogs take one of Desktop's three size presets (`session_view::dialog::DialogSize`:
# normal 480, large 720 × 600, x-large 880 × 680; anomalyco/opencode#63432, whose lint rule is
# `script/ast-grep/dialog-size.yml`) instead of sizing themselves. In every tracked .rs file under
# apps/ and crates/ that draws a dialog (`role("dialog")` or `role("alertdialog")`), it flags a
# dialog-scale literal (380 or more) given to `.w`, `.h`, `.max_w`, `.max_h` or a `W`/`H`/`*_W`/
# `*_H`/`WIDTH`/`HEIGHT` const, and
# a size or edge worked out from the window (`(win_w - 24.0).min(…)`, `(win_h - 480.0) / 2.0`).
# Content inside a dialog stays under 380. A line that really needs one ends in `// dialog-size: ok`.
# Usage: scripts/dialog-size.sh
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

N='(3[89][0-9]|[4-9][0-9]{2}|[0-9]{4,})(\.[0-9]*)?'
PATTERN="\\.(w|h|max_w|max_h)\\(\\s*${N}\\s*\\)|const ([A-Z_]+_)?(W|H|WIDTH|HEIGHT): f32 = ${N}\\s*;|\\((win_w|win_h)\\s*-\\s*[0-9]+(\\.[0-9]*)?\\)\\s*(\\.min\\(|/)"

found=0
while IFS= read -r -d '' f; do
  [ "$f" = crates/session-view/src/dialog.rs ] && continue
  grep -Eq 'role\("(alert)?dialog"\)' "$f" || continue
  if out=$(grep -nE "$PATTERN" "$f" | grep -v '// dialog-size: ok'); then
    while IFS= read -r line; do
      echo "  ERROR  $f:${line%%:*}  choose a DialogSize preset instead of sizing the dialog: ${line#*:}" | sed 's/  */ /g;s/^ /  /'
      found=$((found + 1))
    done <<<"$out"
  fi
done < <(git ls-files -z 'apps/*.rs' 'crates/*.rs')

echo "dialog size: errors: $found"
[ "$found" -eq 0 ]
