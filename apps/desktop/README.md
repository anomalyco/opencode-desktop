# OpenCode (Rune)

- `scripts/dev.sh` builds (dev profile, optimized dependencies; `--release` for release), installs `~/Applications/OpenCode (Rune).app` and relaunches it with `--live`. It skips the relaunch when the binary is unchanged and fails if the running pid doesn't change.
- `scripts/install-autobuild.sh` loads a LaunchAgent that runs `dev.sh --watch` in `~/code/worktrees/seer/desktop-autobuild`: it fetches every 20 s and, when `apps/desktop`, `crates/` or the Cargo files change on `origin/main`, rebuilds and relaunches in the background. A failed build leaves the running app alone.
- Log: `~/Library/Logs/opencode-rune-autobuild.log`. Stop it with `scripts/uninstall-autobuild.sh`.

## Hot editing

- `scripts/hot.sh` opens a separate debug window on the live server (`:4096`) and patches saved edits to function bodies in `src/view.rs` and `src/palette.rs` into it in about 1.5 s, keeping state, tabs and scroll. App flags pass through (`--read-only`, `--server URL`).
- Any other edit (state or type shapes, signatures, other files) restarts the app in a few seconds; it reopens the same tabs and zoom. It never starts a server, and patches never resend a prompt.
- Needs Dioxus CLI 0.7.10 at `~/.cargo/bin/dx` and a Rune checkout at `~/code/open-source/rune` (`RUNE=` overrides). Compiler and app share one HiveMind cargo lease on Studio.
