# Architecture

`rune-desktop` uses **progressive disclosure** in its folder layout so contributors and agents can ship features without navigating core internals.

## 1. Progressive-Disclosure Layout

Start at the top and go deeper only when a feature genuinely needs lower-level control:

```text
rune-desktop/
├── plugins/                    # Tier 1 (Start here): Luau plugins (~80% of features, 0 Rust)
├── features/                   # Tier 2: Decoupled Rust feature crates (parallel, shallow DAG)
│   ├── connect/                #   opencode-feature-connect (provider connect dialog, catalog sync, picker reads, ChatGPT welcome)
│   ├── dictation/              #   opencode-feature-dictation (Hex voice dictation state machine & polling)
│   ├── egg/                    #   opencode-feature-egg (new-session GPU wordmark & brick breaker)
│   ├── mcp/                    #   opencode-feature-mcp (MCP toggle machine, MCPs & OAuth sign-in dialogs)
│   ├── mentions/               #   opencode-feature-mentions (composer @ mentions popover, search machine, badges, session chips, wire encoding)
│   ├── office/                 #   opencode-feature-office (Word/Excel/PPT Vello preview engine)
│   ├── open-in/                #   opencode-feature-open-in (external editor/terminal detection & launch)
│   ├── review/                 #   opencode-feature-review (review changes engine, untracked folders, narrow Changes view, shared file-panel chrome)
│   ├── subagents/              #   opencode-feature-subagents (activity capsule, background shells machine & followers, subagent reply bar)
│   ├── switcher/               #   opencode-feature-switcher (command/file/session palette, shared session search index, project/worktree/branch switcher)
│   ├── tips/                   #   opencode-feature-tips (paced one-time onboarding tip scheduler)
│   ├── updater/                #   opencode-feature-updater (feed, verification, platform apply)
│   └── usage/                  #   opencode-feature-usage (usage limits parser, dialog, picker warnings)
├── crates/                     # Tier 3: Kernel, Runtime, Platform & Embeddable Libraries
│   ├── opencode-code-view/     #   Shared virtualized syntax-highlighted code & diff viewers, review/delimited/label-find data models
│   ├── opencode-kernel/        #   Shell kernel registries (StateSlots, OverlayStack, Scheduler,
│   │                           #   CommandRegistry, EventBus, Feature/Surface traits)
│   ├── opencode-runtime/       #   Headless runtime primitives & explicit state machines (catalog, idle,
│   │                           #   pairing, server pool/form, window life/placement, fork/rewind/pages/queue/send)
│   ├── opencode-ui/            #   Shared UI primitives (widgets, dialog sizing helpers) & shortcut keymap
│   ├── opencode-platform/      #   OS & WASM platform helpers (wall clock, OTLP telemetry, askpass, fetch)
│   ├── session-view/           #   Embeddable timeline (TimelineHost + focused capability sub-traits)
│   ├── opencode-kit/           #   Protocol client, SSE/WS streams, transcript model
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
    ├── desktop/                #   OpenCode (Rune) desktop host organized by domain:
    │   └── src/
    │       ├── main.rs         #     Bootstrap, App state, kernel wiring
    │       ├── kernel.rs       #     Feature & lifecycle registration into opencode-kernel
    │       ├── shell/          #     Window chrome, layout, tabs, input, popups, overlays
    │       ├── session/        #     Session lifecycle, composer, inbox, queue, stream events
    │       ├── workspace/      #     Servers, multi-window peers, projects, storage, launch
    │       ├── surfaces/       #     Host adapters for code-view, files, review, terminal, browser, office, egg
    │       ├── dialogs/        #     Settings, connect, MCP, usage, pickers
    │       ├── plugin_host/    #     Luau runtime & service bindings
    │       └── stories/        #     Visual storybook scenes & demo playback
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

- **No upward dependencies:** `crates/*` never depend on `features/*` or `apps/*`. `features/*` never depend on `apps/*`.
- **No cross-feature dependencies:** Feature crates under `features/*` must not depend on one another. Communicate through `opencode-kernel` (`EventBus`, `CommandRegistry`, `SurfaceRegistry`) or Luau `Contracts`.
- **Non-generic boundaries where possible:** Keep crate boundaries concrete or `dyn` so downstream crates (`apps/desktop`) do not re-monomorphize leaf crate codegen (see [docs/fast-builds.md](docs/fast-builds.md)).
- **Zero tests:** Per `AGENTS.md`, application code is truth; `scripts/arch-check.sh` guards against `#[test]` / `#[cfg(test)]` regressions.
- **File size & folder shape:** Every `.rs` file stays under 800 lines (1,500 hard cap, `scripts/file-size.sh`) and folders stay organized without prefix clutter (`scripts/folder-shape.sh`).
