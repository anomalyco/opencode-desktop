# OpenCode (Rune)

Native macOS desktop client for OpenCode built on [Rune](https://github.com/anomalyco/rune), together with reusable Rust crates so any Rune app (such as Seer) can connect to an OpenCode server and embed live session timelines, ```` ```ui ```` outlines, terminals, or browser panes.

## Quick start

1. Start an OpenCode server locally (either open **OpenCode Desktop**, or run `opencode serve --port 4096` in a terminal).
2. Build and run **OpenCode (Rune)** against that server:
   ```sh
   cargo run -p opencode-desktop -- --live
   ```
   OpenCode (Rune) discovers `http://127.0.0.1:4096` or `~/.config/opencode/service.json` automatically (`--server <url>` or `OPENCODE_SERVER_URL` overrides it) and never starts a server on its own.

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

For prebuilt `main` and side-by-side pull-request `.dmg` builds, see [docs/trying-opencode-rune.md](docs/trying-opencode-rune.md).

## Workspace layout

| Path | Crate | Purpose |
| --- | --- | --- |
| `apps/desktop` | `opencode-desktop` | **OpenCode (Rune)** macOS desktop client |
| `crates/session-view` | `session-view` | Embeddable virtualized OpenCode session timeline view (Markdown, `ui` outlines, Mermaid, tool groups, subagent cards, images, scroll bar) |
| `crates/opencode-kit` | `opencode-kit` | OpenCode HTTP/SSE/WebSocket client, local `service.json` discovery, and session/transcript model |
| `crates/rune-parts` | `rune-parts` | Generative UI (```` ```ui ````) outline parser, catalog, checker, HTML renderer, and optional Rune element view (`features = ["view"]`) |
| `crates/browser-pane` | `browser-pane` | Embeddable Chromium browser pane backed by the out-of-process `OpenCode Browser Engine` sidecar |
| `crates/terminal` | `terminal` | Embeddable Ghostty-VT terminal pane over OpenCode PTY WebSockets |
| `crates/project-icon` | `project-icon` | Procedural CPU-shaded project icons and custom icon loader |
| `crates/hex-client` | `hex-client` | Local HTTP client for driving Hex voice dictation |
| `crates/otlp-lite` | `otlp-lite` | Minimal OTLP/HTTP trace and log exporter (`serde_json` only) |
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
```
