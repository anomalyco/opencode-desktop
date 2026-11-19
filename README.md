# OpenCode (Rune)

Native desktop client (macOS, Windows, and Linux) and `wasm32`/WebGPU browser client for OpenCode built on [Rune](https://github.com/anomalyco/rune), together with reusable Rust crates so any Rune app (such as Seer) can connect to an OpenCode server and embed live session timelines, ```` ```ui ```` outlines, terminals, or browser panes.

## Quick start

1. Start an OpenCode server locally (either open **OpenCode Desktop**, or run `opencode serve --port 4096` in a terminal).
2. Build and run **OpenCode (Rune)** against that server:
   ```sh
   cargo run -p opencode-desktop -- --live
   ```
   OpenCode (Rune) discovers `http://127.0.0.1:4096` or `~/.config/opencode/service.json` automatically (`--server <url>` or `OPENCODE_SERVER_URL` overrides it) and never starts a server on its own.

For fast local or mesh iteration (`scripts/dev-fast.sh` on macOS/Linux, `pwsh scripts/dev-fast.ps1` on Windows, with Luau `--plugin-hot`), see [docs/fast-builds.md](docs/fast-builds.md) (profiles, `sccache`, and fast linkers).

To scaffold a new plugin (`plugins/<id>/`) or Rust feature crate (`features/<id>/`), run `scripts/new-feature.sh` (or `pwsh scripts/new-feature.ps1` on Windows); see [ARCHITECTURE.md](ARCHITECTURE.md) for lane rules and wiring steps.

To run the standalone embeddable timeline viewer and its interactive playground without a server:

```sh
cargo run -p session-view --example read_only
```

To build and install `~/Applications/OpenCode (Rune).app` locally on macOS:

```sh
apps/desktop/scripts/dev.sh            # dev profile (optimized dependencies)
apps/desktop/scripts/dev.sh --release  # release profile
```

To build and serve the browser target (`apps/web`) locally:

```powershell
pwsh scripts/web.ps1
```

A `release` build is what ships: fat LTO and one codegen unit, plus the profile-guided optimization profiles in `apps/desktop/pgo/` on Windows x64 (`pwsh apps/desktop/scripts/release.ps1`) and Apple Silicon macOS (`apps/desktop/scripts/build-app.sh`). A plain `cargo build --release` omits the PGO profile and takes 25–60 minutes and ~10 GB RAM for one `rustc`. Use `--profile release-quick` or `--profile release-thin` for faster optimized iteration (see [docs/fast-builds.md](docs/fast-builds.md)).

For prebuilt `dev` and side-by-side pull-request builds, see [docs/trying-opencode-rune.md](docs/trying-opencode-rune.md). Releases across macOS, Windows, and Linux: [docs/releasing.md](docs/releasing.md).

## Workspace layout

Organized by progressive disclosure in four tiers (full crate inventory, dependency graph, and wiring guide in [ARCHITECTURE.md](ARCHITECTURE.md)):

| Tier | Path | Purpose |
| --- | --- | --- |
| Tier 1 | `plugins/*` | Built-in `--!strict` Luau plugins (panels, dialogs, commands, server sources); hot reloads without touching Rust |
| Tier 2 | `features/*` | Decoupled Rust feature crates (`opencode-feature-*`) with zero cross-feature dependencies and non-generic `&mut dyn *Host` boundaries |
| Tier 3 | `crates/*` | Shell kernel (`opencode-kernel`), UI foundation (`opencode-ui`), headless runtime (`opencode-runtime`), platform/storage/search/i18n/plugin-runtime libraries, and embeddable crates (`session-view`, `opencode-code-view`, `opencode-kit`, `rune-parts`, `terminal`, `browser-pane`, `office-preview`, `project-icon`, `hex-client`, `otlp-lite`) |
| Tier 4 | `apps/*` | Host shells and packaging: `apps/desktop` (`opencode-desktop` library + native binary), `apps/web` (`opencode-web` wasm32/WebGPU host), `apps/installer` (`opencode-rune-setup` Windows bootstrapper), and `apps/browser-engine` (`opencode-browser-engine` CEF sidecar, excluded from the Cargo workspace) |

### Embedding `session-view` in another Rune app

Add git dependencies from this repository (pinned to a commit rev matching your Rune pin):

```toml
opencode-kit = { git = "https://github.com/anomalyco/rune-desktop", rev = "<rev>" }
session-view = { git = "https://github.com/anomalyco/rune-desktop", rev = "<rev>" }
```

Hold a `session_view::ViewState` on your app state (calling `view.setup(waker, headless)` during setup), implement `session_view::TimelineHost` (using `session_view::delegate_view_state!` for the state accessors), drive `session_view::poll(s, now, tx)` each frame, and render `session_view::read_only(s, p, id, transcript)` or `session_view::timeline_area(s, p, id, transcript, pills, header)` alongside `session_view::overlays(s, p, cx)`. See [`crates/session-view/examples/read_only.rs`](crates/session-view/examples/read_only.rs) for a complete minimal host.

## Checks

This repo has no automated tests (see [AGENTS.md](AGENTS.md)); verify changes by running the app and the guardrail scripts (rules detailed in [ARCHITECTURE.md](ARCHITECTURE.md)):

```sh
scripts/file-size.sh
scripts/folder-shape.sh
scripts/dialog-size.sh
scripts/arch-check.sh
```
