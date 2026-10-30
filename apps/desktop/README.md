# OpenCode (Rune)

- `scripts/dev.sh` builds (dev profile, optimized dependencies; `--release` for release), installs `~/Applications/OpenCode (Rune).app` and relaunches it with `--live`. It skips the relaunch when the binary is unchanged and fails if the running pid doesn't change.
- `scripts/install-autobuild.sh` loads a LaunchAgent that runs `dev.sh --watch` in `~/code/worktrees/seer/desktop-autobuild`: it fetches every 20 s and, when `apps/desktop`, `crates/` or the Cargo files change on `origin/main`, rebuilds and relaunches in the background. A failed build leaves the running app alone.
- Log: `~/Library/Logs/opencode-rune-autobuild.log`. Stop it with `scripts/uninstall-autobuild.sh`.
- Server: `--server URL`, else `OPENCODE_SERVER_URL`, else `http://127.0.0.1:4096`. A Mac that reaches another Mac's server through a tunnel sets `OPENCODE_SERVER_URL` for GUI apps with `launchctl setenv` (a login LaunchAgent), so opening the app directly finds it too.

## Shared parts

Use these; don't draw your own. When one lacks what you need, extend it in its module (and its story), never a local copy beside your feature.

| Need | Use (`src/`) |
|---|---|
| A menu, picker or popover | `popover/`: `surface`, `place`, `scrim`, `search_row`, `List` (group headers), `option` (`Opt`), `nav` for the keys. Looks: `MENU` (actions, short choices) and `LIST` (long lists with search). |
| A popover opening and closing | `popover::place` gives it the ⌘⇧K Popover knobs' entrance and exit; commits that open or close one call `popover::motion::opening` / `closing`. Story: Popover › Open and close. |
| A lit row (pointer or keys) | `popover::lit` (`p.row_active`) |
| A group's title | `popover::group_header` |
| A shortcut at a row's end | `popover::hint` (11 pt at 530, faint) |

## Hot editing

- `scripts/hot.sh` opens a separate debug window on the live server (`:4096`) and patches saved edits to function bodies in `src/view.rs` and `src/palette.rs` into it in about 1.5 s, keeping state, tabs and scroll. App flags pass through (`--read-only`, `--server URL`).
- Any other edit (state or type shapes, signatures, other files) restarts the app in a few seconds; it reopens the same tabs and zoom. It never starts a server, and patches never resend a prompt.
- Needs Dioxus CLI 0.7.10 at `~/.cargo/bin/dx` and a Rune checkout at `~/code/open-source/rune` (`RUNE=` overrides). Compiler and app share one HiveMind cargo lease on Studio.
