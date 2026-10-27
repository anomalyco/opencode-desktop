#!/usr/bin/env bash
# Stops and removes the autobuild LaunchAgent. The worktree stays; delete it with git worktree remove.
LABEL=com.kitlangton.opencode-rune-autobuild
launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null
rm -f "$HOME/Library/LaunchAgents/$LABEL.plist"
echo "stopped $LABEL"
