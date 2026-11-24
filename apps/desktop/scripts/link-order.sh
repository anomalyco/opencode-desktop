#!/usr/bin/env bash
# The macOS link of opencode-desktop with the functions a launch runs laid out first and together,
# so a launch whose exe is not in memory reads few pages of it. Cargo calls this as the linker
# (build-app.sh sets CARGO_TARGET_<TRIPLE>_LINKER); every other link goes to cc untouched.
#
# apps/desktop/pgo/startup-order.txt lists those functions in the order a training launch ran them
# (apps/desktop/scripts/startup-order.sh makes it), with the crate hashes taken out of the names:
# a hash changes with every Rune pin, the name without it does not. Here the list is matched
# against the names in the objects being linked, and the linker gets the real names
# (-order_file). A function the list names and this build lacks is skipped. A failure to make the
# order never fails the link: the exe is then linked as it would be without this script.
#
# Beside the exe it writes <exe>.link-order.txt: how many of the listed functions this build has.
set -uo pipefail
list=${OPENCODE_RUNE_ORDER_LIST:-}

# rustc passes a long command line as @file: one argument a line, in double quotes.
args=()
for arg in "$@"; do
  case "$arg" in
    @*)
      while IFS= read -r line; do
        line=${line#\"}
        line=${line%\"}
        line=${line//\\\"/\"}
        args+=("${line//\\\\/\\}")
      done < "${arg#@}"
      ;;
    *) args+=("$arg") ;;
  esac
done

out=
previous=
objects=()
for arg in "${args[@]}"; do
  [ "$previous" = -o ] && out=$arg
  previous=$arg
  case "$arg" in
    *.o | *.rlib | *.a) [ -f "$arg" ] && objects+=("$arg") ;;
  esac
done
case "$(basename "${out:-none}")" in
  opencode-desktop | opencode_desktop-*) ;;
  *) exec cc "$@" ;;
esac
[ -n "$list" ] && [ -s "$list" ] && [ "${#objects[@]}" -gt 0 ] || exec cc "$@"

order=$(mktemp -t opencode-order)
trap 'rm -f "$order"' EXIT
# nm prints an archive's member names and its complaints about members that are not objects:
# only lines that are symbols pass. The two patterns are the list's own (startup-order.sh).
{ nm -j -U "${objects[@]}" 2>/dev/null || true; } | awk -v list="$list" '
  function plain(name) {
    sub(/^_/, "", name)
    sub(/\.llvm\.[0-9]+$/, "", name)
    gsub(/Cs[0-9A-Za-z][0-9A-Za-z]?[0-9A-Za-z]?[0-9A-Za-z]?[0-9A-Za-z]?[0-9A-Za-z]?[0-9A-Za-z]?[0-9A-Za-z]?[0-9A-Za-z]?[0-9A-Za-z]?[0-9A-Za-z]?[0-9A-Za-z]?_/, "C", name)
    gsub(/B[0-9A-Za-z]?[0-9A-Za-z]?[0-9A-Za-z]?_/, "B_", name)
    return name
  }
  BEGIN {
    while ((getline line < list) > 0) if (line != "" && line !~ /^#/ && !(line in rank)) rank[line] = ++count
  }
  /^_/ { key = plain($0); if (key in rank && !($0 in seen)) { seen[$0] = 1; print rank[key] "\t" $0 } }
' | sort -n -s -k1,1 > "$order.ranked"
cut -f2 "$order.ranked" > "$order"
found=$(cut -f1 "$order.ranked" | uniq | wc -l | tr -d ' ')
listed=$(grep -c -v -e '^#' -e '^$' "$list")
rm -f "$order.ranked"
echo "link order: $found of $listed listed functions are in this build ($(wc -l < "$order" | tr -d ' ') symbols)" | tee "$out.link-order.txt" >&2
[ -s "$order" ] || exec cc "$@"
cc "$@" -Wl,-order_file,"$order"
