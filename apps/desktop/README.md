# OpenCode (Rune)

- `scripts/dev.sh` builds (dev profile, optimized dependencies; `--release` for release), installs `~/Applications/OpenCode (Rune).app` and relaunches it with `--live`. It skips the relaunch when the binary is unchanged and fails if the running pid doesn't change.
- `scripts/install-autobuild.sh` loads a LaunchAgent that runs `dev.sh --watch` in `~/code/worktrees/seer/desktop-autobuild`: it fetches every 20 s and, when `apps/desktop`, `crates/` or the Cargo files change on `origin/main`, rebuilds and relaunches in the background. A failed build leaves the running app alone.
- Log: `~/Library/Logs/opencode-rune-autobuild.log`. Stop it with `scripts/uninstall-autobuild.sh`.
