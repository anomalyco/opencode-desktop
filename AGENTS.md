# OpenCode (Rune) (`rune-desktop`)

`anomalyco/rune-desktop` houses **OpenCode (Rune)** (`apps/desktop`) and the shared Rust crates (`crates/*`) that any Rune app (including `anomalyco/seer`) can embed by git dependency.

## Layout

- `apps/desktop` (`opencode-desktop`): the OpenCode (Rune) desktop client.
- `crates/session-view`: embeddable OpenCode session timeline (`TimelineHost`, `ViewState`, `examples/read_only.rs`). Keep it free of `opencode-desktop`-only window/tab/sidebar assumptions so Seer and other Rune apps can embed it directly.
- `crates/opencode-kit`: reusable OpenCode HTTP/SSE/WS client and session/transcript model. Keep it free of app-specific concepts.
- `crates/rune-parts`: ```` ```ui ```` outline catalog, parser, checker, and optional Rune `view` feature (`skills/rune-ui/SKILL.md`).
- `crates/browser-pane` + `apps/browser-engine` (excluded from workspace): embedded Chromium browser pane and its CEF sidecar.
- `crates/terminal`: Ghostty-VT terminal pane over OpenCode PTY WebSockets.
- `crates/project-icon`: procedural CPU-shaded project icons.
- `crates/hex-client`: local Hex voice dictation client.
- `crates/otlp-lite`: lightweight OTLP/HTTP trace and log exporter.

Rune comes from `anomalyco/rune` as a git dependency pinned in the root `Cargo.toml`. Pin only revs on Rune's `dev` branch; upstream fixes go to `anomalyco/rune` first, then bump the rev here.

## Working here

- Run targeted tests for what you touch (`cargo test -p <crate> <filter>`); never hit a real OpenCode server in unit tests (use `opencode_kit::client::Server` mocks or `--fixture`).
- Keep `session-view`'s standalone example building: `cargo check -p session-view --examples`.
- Motion gates: any change to how `apps/desktop` or `crates/session-view` moves must pass `scripts/motion-audit-desktop` and `apps/desktop/scripts/motion-audit.sh` (scenarios in `apps/desktop/src/motion_gate/scenarios.rs` and `apps/desktop/scripts/audit/`).
- File size & folder shape: keep Rust files under 800 non-test lines (1,500 hard cap, `scripts/file-size.sh`) and tests under 1,000 lines; keep prefix families in subfolders (`scripts/folder-shape.sh`).
- Hot reload & local install: `apps/desktop/scripts/dev.sh` (`apps/desktop/scripts/bundle.sh` / `scripts/mac-app`) and `apps/desktop/scripts/hot.sh` (`scripts/rune-hot`).
- Keep personal paths (`/Users/<real-name>`), tailnet hostnames (`*.ts.net`, except `*.example.ts.net` in tests), real session IDs, and credentials out of code, tests, scripts, and commits.

## Explicit state machines

No implicit state machines. Any behaviour with modes or phases is an explicit state machine. Examples: lifecycle, connection and reconnect, streaming, send/queue/steer, drag, popovers and menus, focus, dialogs, quit, update, animation, and request/retry.

- **No encoded state:** don't encode state in combinations of `bool`s, `Option`s, timestamps, counters, `dismissed`/`pending`/`loading` flags or high-water marks.
- **Shape:**
  - One `enum State` whose variants carry only the data valid in that state, so illegal states can't be represented.
  - One transition function, `step(&mut State, Event) -> Effects`. Effects (I/O, timers, redraws, focus moves) are returned for the caller to run, not performed inside the transition.
  - Timers and async completions arrive as events.
- **Official machines:** when the official OpenCode app has a machine for the same behaviour (`packages/app` / `packages/ui` `*-machine.ts`), match its states and transitions, and cite it in a comment.
- **Touched code:** if you touch code that contains an implicit state machine, convert it in the same change.
