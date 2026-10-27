#!/usr/bin/env bash
# Installs and loads the LaunchAgent that rebuilds "OpenCode (Rune)" when origin/main changes.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
WT="$HOME/code/worktrees/seer/desktop-autobuild"
LABEL=com.kitlangton.opencode-rune-autobuild
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
if [ ! -e "$WT/.git" ]; then
  git -C "$REPO" fetch -q origin main
  git -C "$REPO" worktree add --detach "$WT" origin/main
fi
mkdir -p "$HOME/Library/LaunchAgents" "$HOME/Library/Logs"
cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key><array><string>$WT/apps/desktop/scripts/dev.sh</string><string>--watch</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>30</integer>
  <key>StandardOutPath</key><string>$HOME/Library/Logs/opencode-rune-autobuild.log</string>
  <key>StandardErrorPath</key><string>$HOME/Library/Logs/opencode-rune-autobuild.log</string>
</dict></plist>
EOF
launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$PLIST"
echo "loaded $LABEL; log: ~/Library/Logs/opencode-rune-autobuild.log"
