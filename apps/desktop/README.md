# OpenCode (Rune) (`apps/desktop`)

- `cargo run -p opencode-desktop -- --live` runs against a local OpenCode server (`http://127.0.0.1:4096` or `~/.config/opencode/service.json`; override with `--server URL` or `OPENCODE_SERVER_URL`).
- `apps/desktop/scripts/dev.sh` builds (dev profile with optimized dependencies; `--release` for release), installs `~/Applications/OpenCode (Rune).app`, and relaunches it with `--live` when the binary changes.
- `cargo run -p opencode-desktop -- --fixture apps/desktop/assets/fixture.json --scene session` runs offline against the bundled fixture.

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

- `apps/desktop/scripts/hot.sh` opens a debug window on the live server (`:4096`) and patches saved edits to function bodies in `src/view.rs` and `src/palette.rs` into it in about 1.5 s, keeping state, tabs, and scroll. App flags pass through (`--read-only`, `--server URL`).
- Any other edit restarts the app and reopens the same tabs and zoom. Requires Dioxus CLI (`dx`) in `~/.cargo/bin/dx`.
