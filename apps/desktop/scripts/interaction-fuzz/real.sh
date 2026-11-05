#!/usr/bin/env bash
# The interaction fuzzer against a real OpenCode server (t1387):
#
#   real.sh record              # record.ts: what the server streams for the sim's moves
#   real.sh confirm LIST        # replay each "name<TAB>ops" line of LIST on serve.ts's server
#
#   apps/desktop/scripts/interaction-fuzz/real.sh confirm apps/desktop/src/interaction_fuzz/bugs.tsv
#
# OPENCODE_DEV=/path/to/opencode runs Drive against that checkout. Output goes to
# captures/interaction-fuzz.
set -uo pipefail

# DRIVE picks a Drive binary (a newer one installed beside the global, say).
drive="${DRIVE:-opencode-drive}"
command -v "$drive" >/dev/null || { echo "real.sh: no $drive on $(hostname)" >&2; exit 3; }

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../../.." && pwd)"
out="$root/captures/interaction-fuzz"
mkdir -p "$out"
export OPENCODE_DRIVE_MEDIA_DIR="$out/drive-media"
dev=()
if [ -n "${OPENCODE_DEV:-}" ]; then dev=(--dev "$OPENCODE_DEV"); fi

case "${1:-}" in
  check)
    "$drive" --version
    "$drive" check "$here/serve.ts"
    ;;
  record)
    OUT="$out/recorded" "$drive" run "$here/record.ts"
    ;;
  confirm)
    list="${2:?a list of name<TAB>ops lines}"
    server="$out/server.json"
    rm -f "$server"
    "$drive" check "$here/serve.ts" || exit 1
    "$drive" stop --name fuzz-real >/dev/null 2>&1
    # With a script, start stays in the foreground for as long as the script runs.
    FUZZ_REAL_OUT="$server" "$drive" start --name fuzz-real --script "$here/serve.ts" "${dev[@]}" > "$out/drive-start.log" 2>&1 &
    trap '"$drive" stop --name fuzz-real; wait' EXIT
    for _ in $(seq 1 120); do [ -s "$server" ] && break; sleep 1; done
    [ -s "$server" ] || { echo "serve.ts never wrote $server"; exit 1; }
    cd "$root" || exit 1
    cargo test --locked -p opencode-desktop --no-run || exit 1
    rm -f "$out"/real-*.txt
    while IFS=$'\t' read -r name ops; do
      case "$name" in ''|'#'*) continue ;; esac
      FUZZ_REAL="$server" FUZZ_OPS="$ops" FUZZ_NAME="$name" \
        cargo test --locked -p opencode-desktop interaction_fuzz::tests::real_replay -- --ignored --nocapture
    done < "$list"
    grep -h "REPRODUCED" "$out"/real-*.txt
    ;;
  *)
    echo "usage: real.sh check | record | confirm LIST" >&2
    exit 2
    ;;
esac
