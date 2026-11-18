# Architecture

`rune-desktop` uses **progressive disclosure** in its folder layout so contributors and agents can ship features without navigating core internals.

## Verified Architecture Snapshot

- **51 library crates** (`29` non-generic `&mut dyn *Host` feature crates under `features/*` + `22` core/embeddable crates under `crates/*`) plus `3` host app targets (`apps/desktop`, `apps/web`, `apps/installer`), with **0 cycles**.
- **`apps/desktop/src/` reduced from 137.7k → ~57.6k non-blank LOC** (`63.9k` total lines across `5` root files and `7` domain folders), with `struct App` ratcheted at `<= 142` fields.
- **0 files > 800 lines** in `apps/desktop/` and `features/`, **0 folder-shape exceptions**, and **0 `#[test]` / `#[cfg(test)]`** blocks.

## 1. Progressive-Disclosure Layout

Start at the top and go deeper only when a feature genuinely needs lower-level control:

```text
rune-desktop/
├── plugins/                    # Tier 1 (Start here): Luau plugins (~80% of features, 0 Rust)
├── features/                   # Tier 2: Decoupled Rust feature crates (parallel, shallow DAG)
│   ├── annotations/            #   opencode-feature-annotations (timeline/terminal quote toolbar, composer comment cards, quote-comment editor & code/diff line comments)
│   ├── composer-input/         #   opencode-feature-composer-input (attachments, remote upload runner/toast, file dropzone overlay, suggestions machine, history popover, slash commands & prompt history)
│   ├── connect/                #   opencode-feature-connect (provider connect dialog, catalog sync, picker reads, ChatGPT welcome)
│   ├── dictation/              #   opencode-feature-dictation (Hex voice dictation state machine & polling)
│   ├── egg/                    #   opencode-feature-egg (new-session GPU wordmark & brick breaker)
│   ├── files/                  #   opencode-feature-files (workspace file tree, file viewer, Markdown/HTML/table/Mermaid/image/video previews, link resolution & file watcher)
│   ├── find/                   #   opencode-feature-find (session timeline Find state machine, incremental transcript search index, match highlighting & find bar)
│   ├── history/                #   opencode-feature-history (session timeline message navigation, Jump to message, Rewind, and Fork)
│   ├── inbox/                  #   opencode-feature-inbox (session prompt queue strip, drag/edit/rewrite state machines, durable prompt outbox & delivery receipt checks)
│   ├── mcp/                    #   opencode-feature-mcp (MCP toggle machine, MCPs & OAuth sign-in dialogs)
│   ├── mentions/               #   opencode-feature-mentions (composer @ mentions popover, search machine, badges, session chips, wire encoding)
│   ├── narrow/                 #   opencode-feature-narrow (narrow-window <768px layout mode, view switcher, tabs/More drawers & draggable sheets)
│   ├── notifications/          #   opencode-feature-notifications (system notifications, actionable permission/question toasts, coordinator & activation)
│   ├── office/                 #   opencode-feature-office (Word/Excel/PPT Vello preview engine)
│   ├── open-in/                #   opencode-feature-open-in (external editor/terminal detection & launch)
│   ├── panels/                 #   opencode-feature-panels (session side/split/bottom panel strip, drag/resize/closing machines, launchers, menus & persistence)
│   ├── projects/               #   opencode-feature-projects (project tracking, Home across servers, session import & remote directory picker dialog)
│   ├── restore/                #   opencode-feature-restore (window state restoration, composer drafts, blob attachments, timeline places, boot previews, offline copies & draft undo)
│   ├── review/                 #   opencode-feature-review (review changes engine, untracked folders, narrow Changes view, focus anchoring)
│   ├── session-menu/           #   opencode-feature-session-menu (session context menus, inline & dialog rename, undoable delete, export & read/mute/hide preferences)
│   ├── settings/               #   opencode-feature-settings (Settings two-pane shell, preferences persistence, search & all built-in settings pages)
│   ├── subagents/              #   opencode-feature-subagents (activity capsule, background shells machine & followers, subagent reply bar)
│   ├── switcher/               #   opencode-feature-switcher (command/file/session palette, shared session search index, project/worktree/branch switcher)
│   ├── terminal-surface/       #   opencode-feature-terminal-surface (Ghostty-VT terminal pane, workspace tabs, persistence, shell integration, find, links & paste guard)
│   ├── timeline-extras/        #   opencode-feature-timeline-extras (todo pill/list, permission/question reply machines, live subagent step rows, auto-accept & timeline capture fixtures)
│   ├── tips/                   #   opencode-feature-tips (paced one-time onboarding tip scheduler)
│   ├── updater/                #   opencode-feature-updater (feed, verification, platform apply)
│   ├── usage/                  #   opencode-feature-usage (usage limits parser, dialog, picker warnings)
│   └── workspace-tools/        #   opencode-feature-workspace-tools (new-worktree branch picker, per-folder rune client MCP hub, Home session search & local full-text search)
├── crates/                     # Tier 3: Kernel, Runtime, Platform & Embeddable Libraries
│   ├── opencode-code-view/     #   Shared virtualized syntax-highlighted code & diff viewers, review/delimited/label-find data models
│   ├── opencode-kernel/        #   Shell kernel registries (StateSlots, OverlayStack, Scheduler,
│   │                           #   CommandRegistry, EventBus, Feature/Surface traits)
│   ├── opencode-runtime/       #   Headless runtime primitives & explicit state machines (catalog, idle,
│   │                           #   pairing, server pool/form, window life/placement, fork/rewind/pages/queue/send)
│   ├── opencode-ui/            #   Shared UI primitives (`widgets`, dialog sizing), `keymap`, `panel_chrome`, `popup` menus & `toast` notifications
│   ├── opencode-platform/      #   OS & WASM platform helpers (wall clock, OTLP telemetry, askpass, fetch)
│   ├── session-view/           #   Embeddable timeline (TimelineHost + focused capability sub-traits)
│   ├── opencode-kit/           #   Protocol client, SSE/WS streams, transcript model
│   ├── opencode-plugin-runtime/#   App-independent Luau plugin runtime primitives (catalogs, icons, images, flyout, machines, processes, power, QR, hot reload, sources, tasks)
│   ├── opencode-plugin-spec/   #   Single-source-of-truth contract.json + Luau/Rust codegen
│   ├── opencode-storage/       #   SQLite/web storage engine
│   ├── opencode-search/        #   Headless full-text, fuzzysort, path index, file ranking & URL query primitives (#![forbid(unsafe_code)])
│   ├── opencode-i18n/          #   Localization catalogs & pluralization
│   ├── browser-pane/           #   Embedded Chromium sidecar surface
│   ├── terminal/               #   Ghostty-VT PTY surface
│   ├── office-preview/         #   BetterOffice parsing & display-list engine
│   ├── rune-parts/             #   Generative UI (```ui) parser & Rune renderer
│   ├── project-icon/           #   Procedural project icons
│   ├── hex-client/             #   Hex voice dictation client
│   ├── otlp-lite/              #   Lightweight OTLP/HTTP trace exporter
│   ├── ureq-web/               #   WASM ureq-compatible HTTP shim
│   ├── win-install/            #   Windows install/update identity & resources
│   └── authenticode/           #   Windows Authenticode verification
└── apps/                       # Tier 4: Thin Entrypoints & Host Shells
    ├── desktop/                #   OpenCode (Rune) desktop host organized by domain (~63.9k LOC / ~59.3k excl. stories):
    │   └── src/
    │       ├── main.rs         #     Process bootstrap, allocator, fonts, window hooks
    │       ├── app.rs          #     App state & lifecycle (fields ratcheted <= 142)
    │       ├── tick.rs         #     4-phase tick scheduler & setup/visibility hooks
    │       ├── kernel.rs       #     Feature & lifecycle registration into opencode-kernel
    │       ├── exports.rs      #     Domain module compatibility re-export index
    │       ├── shell/          #     Window chrome, layout, tabs, input, popups, overlays
    │       ├── session/        #     Session lifecycle, composer, inbox, queue, stream events
    │       ├── workspace/      #     Servers, multi-window peers, projects, storage, launch
    │       ├── surfaces/       #     Thin host adapters for code-view, files, review, terminal, browser, office, egg
    │       ├── dialogs/        #     Thin host adapters for settings, connect, MCP, usage, pickers
    │       ├── plugin_host/    #     Luau runtime & service bindings
    │       └── stories/        #     Visual storybook scenes & demo playback (gated by `stories` feature)
    ├── web/                    #   WASM / WebGPU browser target
    ├── installer/              #   Windows setup bootstrapper
    └── browser-engine/         #   Out-of-process CEF sidecar bundle
```

## 2. The Three-Lane Feature Rule

Pick the first lane that fits your feature. Do not add fields to `struct App` or edit central dispatch chains unless a feature genuinely belongs in the shell kernel.

| Lane | Location | When to use | Compile cost |
| --- | --- | --- | --- |
| **Lane A (Default, ~80%)** | `plugins/<id>/` | Dialogs, panels, settings pages, composer pills, popovers, commands, slash actions, MCP tools, server sources | 0 ms Rust (Luau hot reload) |
| **Lane B (~15%)** | `features/<id>/` | GPU shaders, frame recorders, heavy local indexing, OS-specific engines, or sub-ms hot paths | Leaf crate only |
| **Lane C (~5%)** | `crates/<id>/` | Reusable libraries or embeddable surfaces shared with `anomalyco/seer` or other Rune apps | Leaf crate only |

Scaffold a new feature into its lane (`scripts/new-feature.sh` on macOS/Linux, `pwsh scripts/new-feature.ps1` on Windows):

```sh
scripts/new-feature.sh plugin <id>    # Lane A (default): Luau plugin ready for --plugin-hot
scripts/new-feature.sh feature <id>   # Lane B: decoupled Rust feature crate + kernel wiring
```

## 3. Dependency Rules (Enforced by `scripts/arch-check.sh`)

Dependencies flow strictly downward so the workspace stays **wide and shallow** for parallel Rust compilation:

```text
apps/*  ──▶  features/*  ──▶  crates/opencode-kernel, crates/opencode-platform, crates/*
  │                              ▲
  └──────────────────────────────┘
```

- **No upward or cyclic dependencies (Rules 1–2):** `crates/*` never depend on `features/*` or `apps/*`; `features/*` never depend on `apps/*` or on sibling `features/*`; full workspace DFS confirms zero cycles across all 54 workspace members (29 features, 22 core crates, 3 apps).
- **Core purity & `#![forbid(unsafe_code)]` (Rules 3 & 9):** `opencode-kernel`, `opencode-runtime`, `opencode-ui`, `opencode-search`, and all 25 pure feature crates forbid `unsafe_code` (only the 4 FFI/GPU features `egg`, `office`, `open-in`, `updater` are allowlisted).
- **Non-generic boundaries:** Keep crate boundaries concrete or `&mut dyn *Host` so downstream crates (`apps/desktop`) do not re-monomorphize leaf crate codegen (see [docs/fast-builds.md](docs/fast-builds.md)).
- **`struct App` ratchet & root cleanliness (Rules 4–5):** `apps/desktop/src/` stays at `<= 10` root files and `struct App` is ratcheted at `<= 142` fields (new feature state belongs in `opencode_kernel::StateSlots` or a Luau plugin).
- **Web parity & zero tests (Rules 6–7):** `apps/web/Cargo.toml` stays synced with shared desktop workspace dependencies, and `#[test]` / `#[cfg(test)]` are rejected per `AGENTS.md`.
- **Hard file-size ceiling & zero folder-shape exceptions (Rule 8):** Every tracked `.rs` file in `apps/desktop/` and `features/` is hard-capped at `<= 800` lines, and `scripts/folder-shape.sh` runs with zero exceptions.
