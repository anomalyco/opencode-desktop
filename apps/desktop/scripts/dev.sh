#!/usr/bin/env bash
# One-command dev build of "OpenCode (Rune).app".
#   scripts/dev.sh              build (dev profile), install, relaunch with --live
#   scripts/dev.sh --release    same with the release profile
#   scripts/dev.sh --watch      follow origin/main in the autobuild worktree and rebuild on change
# Builds run through HiveMind on Studio. The relaunch is skipped when the binary is unchanged.
set -uo pipefail
export PATH="$HOME/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
APP="$HOME/Applications/OpenCode (Rune).app"
INSTALLED="$APP/Contents/MacOS/opencode-desktop"
STAMP="$HOME/.cache/opencode-rune-build-sha"
LOG="$HOME/Library/Logs/opencode-rune-autobuild.log"
WATCH_DIR="${AUTOBUILD_DIR:-$HOME/code/worktrees/seer/desktop-autobuild}"
PROFILE=debug; CARGO_FLAGS=(); WATCH=0
for a in "$@"; do
  case "$a" in
    --release) PROFILE=release; CARGO_FLAGS=(--release) ;;
    --watch) WATCH=1 ;;
    *) echo "unknown argument: $a" >&2; exit 2 ;;
  esac
done

say() { echo "$(date '+%F %T') $*"; }
pids() { local p; for p in $(pgrep -x opencode-desktop); do ps -o command= -p "$p" | grep -qF "$INSTALLED" && echo "$p"; done | sort | tr '\n' ' '; }

overlaps() { local p; for p in $1; do case " $2 " in *" $p "*) return 0;; esac; done; return 1; }

# build_install <root>: build, fetch, install, relaunch. Returns non-zero on any failure.
build_install() {
  local root="$1" start=$SECONDS bin sha
  bin="$root/target/$PROFILE/opencode-desktop"
  ( cd "$root" && hivemind run --kind cargo --worker studio --fetch "target/$PROFILE/opencode-desktop" -- \
      cargo build -p opencode-desktop ${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"} ) || { say "build failed"; return 1; }
  test -x "$bin" || { say "no binary at $bin after build"; return 1; }
  sha=$(shasum "$bin" | cut -d" " -f1)
  if [ "$(cat "$STAMP" 2>/dev/null)" = "$sha" ] && [ -n "$(pids)" ]; then
    say "binary unchanged, not relaunching ($((SECONDS - start))s)"; return 0
  fi
  local before; before="$(pids)" flags=""
  # Unattended: don't restart under Kit's hands (the app saves its session and drafts anyway).
  [ "$WATCH" = 1 ] && flags="--wait-idle 20"
  MAC_APP_FLAGS="$flags" "$root/apps/desktop/scripts/bundle.sh" "$bin" --live >/dev/null || { say "bundle.sh failed"; return 1; }
  mkdir -p "$(dirname "$STAMP")"; echo "$sha" > "$STAMP"
  local i after
  for i in $(seq 1 50); do
    after="$(pids)"
    if [ -n "$after" ] && ! overlaps "$before" "$after"; then
      say "relaunched: pid ${before:-none} -> $after ($((SECONDS - start))s)"; return 0
    fi
    sleep 0.2
  done
  say "FAILED: running pid did not change after relaunch (before: ${before:-none}, now: ${after:-none})"
  return 1
}

if [ "$WATCH" = 0 ]; then
  build_install "$ROOT" || exit 1
  echo "done in ${SECONDS}s"
  exit 0
fi

exec >>"$LOG" 2>&1
test -d "$WATCH_DIR/.git" -o -f "$WATCH_DIR/.git" || { say "missing worktree $WATCH_DIR (run install-autobuild.sh)"; exit 1; }
cd "$WATCH_DIR" || exit 1
say "watching origin/main in $WATCH_DIR ($PROFILE)"
failed=""
first=1
while true; do
  if ! git fetch -q origin main; then say "fetch failed"; sleep 20; continue; fi
  head=$(git rev-parse HEAD); new=$(git rev-parse origin/main)
  if [ "$head" != "$new" ] || [ $first = 1 ]; then
    changed=$(git diff --name-only "$head" "$new" -- apps/desktop crates Cargo.toml Cargo.lock)
    if [ "$head" != "$new" ]; then
      git checkout -q --detach "$new" || { say "checkout failed"; sleep 20; continue; }
      say "main moved ${head:0:8} -> ${new:0:8}"
      # the script may have changed under us; restart from the new copy
      if [ "$(git diff --name-only "$head" "$new" -- apps/desktop/scripts/dev.sh)" ]; then exec "$WATCH_DIR/apps/desktop/scripts/dev.sh" "$@"; fi
    fi
    if { [ $first = 1 ] || [ -n "$changed" ]; } && [ "$failed" != "$new" ]; then
      build_install "$WATCH_DIR" || failed="$new"
    fi
    first=0
  fi
  sleep 20
done
