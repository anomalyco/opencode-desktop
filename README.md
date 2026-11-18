# OpenCode (Rune)

Native macOS desktop client for OpenCode built on [Rune](https://github.com/anomalyco/rune), together with reusable Rust crates so any Rune app (such as Seer) can connect to an OpenCode server and embed live session timelines, ```` ```ui ```` outlines, terminals, or browser panes.

## Quick start

1. Start an OpenCode server locally (either open **OpenCode Desktop**, or run `opencode serve --port 4096` in a terminal).
2. Build and run **OpenCode (Rune)** against that server:
   ```sh
   cargo run -p opencode-desktop -- --live
   ```
   OpenCode (Rune) discovers `http://127.0.0.1:4096` or `~/.config/opencode/service.json` automatically (`--server <url>` or `OPENCODE_SERVER_URL` overrides it) and never starts a server on its own.

For the fastest local/mesh edit-compile-run loop (`dev-fast` profile, skipping Office/Vello and standalone story modules, Luau hot reload), run `scripts/dev-fast.sh` (macOS/Linux) or `pwsh scripts/dev-fast.ps1` (Windows). See [docs/fast-builds.md](docs/fast-builds.md) for the profile matrix, `sccache`, and fast linkers.

To scaffold a new feature in the right lane (or `pwsh scripts/new-feature.ps1` on Windows):

```sh
scripts/new-feature.sh plugin <id>    # Lane A (default): Luau plugin, hot-reloaded with --plugin-hot
scripts/new-feature.sh feature <id>   # Lane B: decoupled Rust feature crate
```

To run the standalone embeddable timeline viewer and its interactive playground without a server:

```sh
cargo run -p session-view --example read_only
```

To build and install `~/Applications/OpenCode (Rune).app` locally:

```sh
apps/desktop/scripts/dev.sh            # dev profile (optimized dependencies)
apps/desktop/scripts/dev.sh --release  # release profile
```

A release build is what ships: the whole program optimised as one (fat LTO, one codegen unit) and, on
Windows x64, with the profile-guided optimisation profile in `apps/desktop/pgo/`. Build and time it
with `pwsh apps/desktop/scripts/release.ps1` (a plain `cargo build --release` leaves the profile out;
it takes 25 to 60 minutes and about 10 GB for one rustc). `--profile release-quick` builds in minutes
for iterating.

For prebuilt `dev` and side-by-side pull-request `.dmg` builds, see [docs/trying-opencode-rune.md](docs/trying-opencode-rune.md). Releases: [docs/releasing.md](docs/releasing.md).

## Workspace layout

Organized by progressive disclosure (see [ARCHITECTURE.md](ARCHITECTURE.md)):

| Path | Crate / Scope | Purpose |
| --- | --- | --- |
| `plugins/*` | Built-in Luau plugins | Default home for ~80% of UI features (panels, dialogs, commands, server sources); hot reloads without touching Rust |
| `features/annotations` | `opencode-feature-annotations` | Timeline/terminal quote toolbar, composer comment cards, floating quote-comment editor, and code/diff line comments |
| `features/connect` | `opencode-feature-connect` | Provider and integration connect dialog, catalog sync, picker catalog reads, and ChatGPT plan welcome |
| `features/dictation` | `opencode-feature-dictation` | Hex voice dictation state machine, availability polling, and composer insertion |
| `features/egg` | `opencode-feature-egg` | New-session GPU wordmark & Brick Breaker Easter egg |
| `features/files` | `opencode-feature-files` | Workspace file tree, file viewer, Markdown/HTML/table/Mermaid/image/video previews, link resolution, and file watcher engine |
| `features/find` | `opencode-feature-find` | Session timeline Find (`Cmd/Ctrl+F`) state machine, incremental transcript search index, match highlighting, and find bar |
| `features/history` | `opencode-feature-history` | Session timeline message navigation, Jump to message dialog, Rewind (`/undo`, `/redo`), and Fork from message dialog |
| `features/inbox` | `opencode-feature-inbox` | Session prompt queue strip, drag/edit/rewrite state machines, durable prompt outbox, and delivery receipt checks |
| `features/mcp` | `opencode-feature-mcp` | MCP server toggle machine, MCPs dialog, and browser OAuth sign-in dialog |
| `features/mentions` | `opencode-feature-mentions` | Composer `@` mentions popover, search state machine, badges, session chips, and wire encoding |
| `features/narrow` | `opencode-feature-narrow` | Narrow-window (<768 px) layout mode, view switcher, tabs/More drawers, and draggable sheets |
| `features/notifications` | `opencode-feature-notifications` | System notifications, actionable permission/question toasts, coordinator, and activation helpers |
| `features/office` | `opencode-feature-office` | Word, Excel, and PowerPoint Vello preview engine |
| `features/open-in` | `opencode-feature-open-in` | External editor/terminal detection, launching, preferred app persistence, and URL opening |
| `features/panels` | `opencode-feature-panels` | Session side, split, and bottom panel strip, drag/resize/closing state machines, launchers, menus, and persistence |
| `features/review` | `opencode-feature-review` | Review changes engine, untracked folder loader, narrow Changes view, and focus anchoring |
| `features/session-menu` | `opencode-feature-session-menu` | Session right-click context menus, inline and dialog rename, undoable delete, export, and read/mute/hide preferences |
| `features/settings` | `opencode-feature-settings` | Settings two-pane shell, preferences persistence, search index, and all built-in settings pages |
| `features/subagents` | `opencode-feature-subagents` | Running-work activity capsule, background shells state machine & output followers, and subagent reply bar |
| `features/switcher` | `opencode-feature-switcher` | Command/file/session palette switcher, shared session search index, and project/worktree/branch switcher |
| `features/terminal-surface` | `opencode-feature-terminal-surface` | Ghostty-VT terminal pane, workspace tabs, persistence, shell integration, inline rename, find, links, and paste guard |
| `features/tips` | `opencode-feature-tips` | Paced one-time onboarding tip state machine and persistence |
| `features/updater` | `opencode-feature-updater` | Release feed, verification, and platform update engine |
| `features/usage` | `opencode-feature-usage` | Usage limits parser, Usage dialog, and model-picker budget warnings |
| `crates/opencode-code-view` | `opencode-code-view` | Shared virtualized syntax-highlighted code and diff viewers plus review, delimited table, and label-find data models |
| `crates/opencode-kernel` | `opencode-kernel` | Shell kernel registries (`StateSlots`, `OverlayStack`, `LifecycleScheduler`, `CommandRegistry`, `EventBus`) |
| `crates/opencode-runtime` | `opencode-runtime` | Headless runtime primitives and explicit state machines (catalog, idle, pairing, server pool/form, window life/placement, fork/rewind/pages/queue/send) |
| `crates/opencode-search` | `opencode-search` | Headless full-text, fuzzysort, path index, file ranking, and URL query primitives (`#![forbid(unsafe_code)]`) |
| `crates/opencode-ui` | `opencode-ui` | Shared OpenCode UI primitives (`widgets`, dialog sizing helpers), shortcut keymap (`keymap`), file/review panel chrome (`panel_chrome`), popup/context menu engine (`popup`), and toast notification stack (`toast`) |
| `crates/opencode-platform` | `opencode-platform` | Cross-platform OS/WASM utilities (wall clock, OTLP telemetry, askpass, HTTP fetch) |
| `crates/session-view` | `session-view` | Embeddable virtualized OpenCode session timeline view (Markdown, `ui` outlines, Mermaid, tool groups, subagent cards, images, scroll bar) |
| `crates/opencode-kit` | `opencode-kit` | OpenCode HTTP/SSE/WebSocket client, local `service.json` discovery, and session/transcript model |
| `crates/rune-parts` | `rune-parts` | Generative UI (```` ```ui ````) outline parser, catalog, checker, HTML renderer, and optional Rune element view (`features = ["view"]`) |
| `crates/browser-pane` | `browser-pane` | Embeddable Chromium browser pane backed by the out-of-process `OpenCode Browser Engine` sidecar |
| `crates/terminal` | `terminal` | Embeddable Ghostty-VT terminal pane over OpenCode PTY WebSockets |
| `crates/project-icon` | `project-icon` | Procedural CPU-shaded project icons and custom icon loader |
| `crates/hex-client` | `hex-client` | Local HTTP client for driving Hex voice dictation |
| `crates/otlp-lite` | `otlp-lite` | Minimal OTLP/HTTP trace and log exporter (`serde_json` only) |
| `apps/desktop` | `opencode-desktop` | **OpenCode (Rune)** native desktop client (`shell/`, `session/`, `workspace/`, `surfaces/`, `dialogs/`, `plugin_host/`, `stories/`) |
| `apps/web` | `opencode-web` | WASM / WebGPU browser client |
| `apps/browser-engine` | `opencode-browser-engine` | Standalone CEF sidecar bundle (`OpenCode Browser Engine.app`), excluded from the default Cargo workspace |

### Embedding `session-view` in another Rune app

Add git dependencies from this repository (pinned to a commit rev matching your Rune pin):

```toml
opencode-kit = { git = "https://github.com/anomalyco/rune-desktop", rev = "<rev>" }
session-view = { git = "https://github.com/anomalyco/rune-desktop", rev = "<rev>" }
```

Hold a `session_view::ViewState` on your app state, implement `session_view::TimelineHost` (using `session_view::delegate_view_state!` for the state accessors), and render `session_view::timeline::session_timeline`. See [`crates/session-view/examples/read_only.rs`](crates/session-view/examples/read_only.rs) for a complete minimal host.

## Checks

This repo has no automated tests (see `AGENTS.md`); verify changes by running the app.

```sh
scripts/file-size.sh
scripts/folder-shape.sh
scripts/dialog-size.sh
scripts/arch-check.sh
```
