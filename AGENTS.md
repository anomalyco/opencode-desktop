# OpenCode (Rune) (`rune-desktop`)

`anomalyco/rune-desktop` houses **OpenCode (Rune)** (`apps/desktop`) and the shared Rust crates (`crates/*`) that any Rune app (including `anomalyco/seer`) can embed by git dependency.

## Layout (Progressive Disclosure)

Start at the top and go deeper only when a feature needs lower-level control (see [`ARCHITECTURE.md`](ARCHITECTURE.md)):

- **`plugins/`** (Tier 1 — default for ~80% of features): Luau plugins compiled against `opencode-plugin-spec`, hot-reloaded in place with zero Rust edits.
- **`features/`** (Tier 2 — decoupled Rust feature crates):
  - `features/annotations` (`opencode-feature-annotations`): timeline/terminal quote toolbar, composer comment cards, floating quote-comment editor, and code/diff line comments.
  - `features/connect` (`opencode-feature-connect`): provider connect dialog, catalog sync, picker reads, and ChatGPT plan welcome.
  - `features/dictation` (`opencode-feature-dictation`): Hex voice dictation state machine & polling.
  - `features/egg` (`opencode-feature-egg`): new-session GPU wordmark & Brick Breaker Easter egg.
  - `features/files` (`opencode-feature-files`): workspace file tree, file viewer, Markdown/HTML/table/Mermaid/image/video previews, link resolution, and file watcher engine.
  - `features/find` (`opencode-feature-find`): session timeline Find state machine, incremental transcript search index, match highlighting, and find bar.
  - `features/history` (`opencode-feature-history`): session timeline message navigation, Jump to message dialog, Rewind, and Fork from message dialog.
  - `features/inbox` (`opencode-feature-inbox`): session prompt queue strip, drag/edit/rewrite state machines, durable prompt outbox, and delivery receipt checks.
  - `features/mcp` (`opencode-feature-mcp`): MCP server toggle machine, MCPs dialog, and browser OAuth sign-in dialog.
  - `features/mentions` (`opencode-feature-mentions`): composer `@` mentions popover, search state machine, badges, session chips, and wire encoding.
  - `features/narrow` (`opencode-feature-narrow`): narrow-window (<768 px) layout mode, view switcher, tabs/More drawers, and draggable sheets.
  - `features/notifications` (`opencode-feature-notifications`): system notifications, actionable permission/question toasts, coordinator, and activation helpers.
  - `features/office` (`opencode-feature-office`): Word, Excel, and PowerPoint Vello preview engine.
  - `features/open-in` (`opencode-feature-open-in`): external editor/terminal detection & launch.
  - `features/panels` (`opencode-feature-panels`): session side, split, and bottom panel strip, drag/resize/closing state machines, launchers, menus, and persistence.
  - `features/projects` (`opencode-feature-projects`): project tracking, Home across servers, session JSON import, sidebar organize/customize/recent state, and remote directory picker dialog.
  - `features/restore` (`opencode-feature-restore`): window state restoration, composer drafts, blob attachments, timeline places, boot previews, offline transcript copies, and draft undo persistence.
  - `features/review` (`opencode-feature-review`): review changes engine, untracked folder loader, narrow Changes view, and focus anchoring.
  - `features/session-menu` (`opencode-feature-session-menu`): session right-click context menus, inline and dialog rename, undoable delete, export, and read/mute/hide preferences.
  - `features/settings` (`opencode-feature-settings`): Settings two-pane shell, preferences persistence, search index, and all built-in settings pages.
  - `features/subagents` (`opencode-feature-subagents`): running-work activity capsule, background shells state machine & output followers, and subagent reply bar.
  - `features/switcher` (`opencode-feature-switcher`): command/file/session palette switcher, shared session search index, and project/worktree/branch switcher.
  - `features/terminal-surface` (`opencode-feature-terminal-surface`): Ghostty-VT terminal pane, workspace tabs, persistence, shell integration, inline rename, find, links, and paste guard.
  - `features/tips` (`opencode-feature-tips`): paced one-time onboarding tip scheduler.
  - `features/updater` (`opencode-feature-updater`): release feed, verification, and platform update engine.
  - `features/usage` (`opencode-feature-usage`): usage limits parser, Usage dialog, and picker warnings.
- **`crates/`** (Tier 3 — kernel, platform, and embeddable libraries):
  - `crates/opencode-code-view`: shared virtualized syntax-highlighted code & diff viewers plus review, delimited table, and label-find data models.
  - `crates/opencode-kernel`: shell kernel registries (`StateSlots`, `OverlayStack`, `LifecycleScheduler`, `CommandRegistry`, `EventBus`, `Feature` / `SurfaceProvider`).
  - `crates/opencode-search`: headless full-text, fuzzysort, path index, file ranking, and URL query primitives (`#![forbid(unsafe_code)]`).
  - `crates/opencode-ui`: shared OpenCode UI primitives (`widgets`, dialog sizing), shortcut `keymap`, file/review `panel_chrome`, `popup` menu engine, and `toast` notifications (`#![forbid(unsafe_code)]`).
  - `crates/opencode-platform`: OS and WASM platform helpers (wall clock, OTLP telemetry, askpass, HTTP fetch).
  - `crates/opencode-plugin-runtime`: reusable App-independent Luau plugin runtime primitives (catalogs, console, flyout, hot reload, icons, images, machines, power, processes, QR, redraw, reveal, sources, tasks).
  - `crates/session-view`: embeddable OpenCode session timeline (`TimelineHost` + focused capability sub-traits in `traits.rs`, `ViewState`, `examples/read_only.rs`). Keep it free of `opencode-desktop`-only window/tab/sidebar assumptions so Seer and other Rune apps can embed it directly.
  - `crates/opencode-kit`: reusable OpenCode HTTP/SSE/WS client and session/transcript model. Keep it free of app-specific concepts.
  - `crates/rune-parts`: ```` ```ui ```` outline catalog, parser, checker, and optional Rune `view` feature (`skills/rune-ui/SKILL.md`).
  - `crates/browser-pane` + `apps/browser-engine` (excluded from workspace): embedded Chromium browser pane and its CEF sidecar.
  - `crates/terminal`: Ghostty-VT terminal pane over OpenCode PTY WebSockets.
  - `crates/project-icon`: procedural CPU-shaded project icons.
  - `crates/hex-client`: local Hex voice dictation client.
  - `crates/otlp-lite`: lightweight OTLP/HTTP trace and log exporter.
- **`apps/`** (Tier 4 — host shells & installers):
  - `apps/desktop` (`opencode-desktop`): the OpenCode (Rune) desktop client organized by domain (`shell/`, `session/`, `workspace/`, `surfaces/`, `dialogs/`, `plugin_host/`, `stories/`).
  - `apps/web` (`opencode-web`): WASM / WebGPU browser host.
  - `apps/installer`: Windows setup bootstrapper.

Rune comes from `anomalyco/rune` as a git dependency pinned in the root `Cargo.toml`. Pin only revs on Rune's `dev` branch; upstream fixes go to `anomalyco/rune` first, then bump the rev here.

## No tests

Application code only. This repo has no automated tests of any kind, and none get added back (Hona and Kit, Nov 10: "Application code is truth, tests are a poor replica"):

- No unit tests, `#[test]` or `#[cfg(test)]` code, `tests.rs` / `*_tests.rs` files, mocks, fuzzers, golden or snapshot files, motion audits, test benchmarks or CI test steps.
- Don't spend time writing, fixing or running tests. Verify a change by building and running the app (`--fixture`, a story, or live).
- Stories and `--fixture` are app features for looking at the app, not tests: don't turn them into assertions.

## Working here

- Keep `session-view`'s standalone example building: `cargo check -p session-view --examples`.
- File size, folder shape & architecture guardrails: keep Rust files under 800 lines (1,500 hard cap, `scripts/file-size.sh`); keep prefix families in subfolders (`scripts/folder-shape.sh`); keep crate dependency direction and root breadth clean (`scripts/arch-check.sh`).
- Dialog sizes: every dialog takes a `session_view::dialog::DialogSize` preset (normal 480, large 720 × 600, x-large 880 × 680; `widgets::sized` / `sized_modal` in the app), never its own width or height (`scripts/dialog-size.sh`).
- Hot reload & local install: `apps/desktop/scripts/dev.sh` (`apps/desktop/scripts/bundle.sh` / `scripts/mac-app`) and `apps/desktop/scripts/hot.sh` (`scripts/rune-hot`).
- Keep personal paths (`/Users/<real-name>`), tailnet hostnames (`*.ts.net`, except `*.example.ts.net`), real session IDs, and credentials out of code, scripts, and commits.

## Explicit state machines

No implicit state machines. Any behaviour with modes or phases is an explicit state machine. Examples: lifecycle, connection and reconnect, streaming, send/queue/steer, drag, popovers and menus, focus, dialogs, quit, update, animation, and request/retry.

- **No encoded state:** don't encode state in combinations of `bool`s, `Option`s, timestamps, counters, `dismissed`/`pending`/`loading` flags or high-water marks.
- **Shape:**
  - One `enum State` whose variants carry only the data valid in that state, so illegal states can't be represented.
  - One transition function, `step(&mut State, Event) -> Effects`. Effects (I/O, timers, redraws, focus moves) are returned for the caller to run, not performed inside the transition.
  - Timers and async completions arrive as events.
- **Official machines:** when the official OpenCode app has a machine for the same behaviour (`packages/app` / `packages/ui` `*-machine.ts`), match its states and transitions, and cite it in a comment.
- **Touched code:** if you touch code that contains an implicit state machine, convert it in the same change.
