<p align="center">
  <a href="https://opencode.ai">
    <picture>
      <source srcset="https://opencode.ai/brand/files/opencode-wordmark-transparent-dark.svg" media="(prefers-color-scheme: dark)">
      <source srcset="https://opencode.ai/brand/files/opencode-wordmark-transparent-light.svg" media="(prefers-color-scheme: light)">
      <img src="https://opencode.ai/brand/files/opencode-wordmark-transparent-light.svg" alt="OpenCode" height="44">
    </picture>
  </a>
</p>

<h3 align="center">OpenCode (Rune)</h3>

<p align="center">
  OpenCode Desktop, rewritten from scratch in Rust on <a href="https://github.com/anomalyco/rune">Rune</a>.<br>
  10–100× faster and lighter. Native on macOS, Windows, and Linux — and the same app in the browser over WebGPU.
</p>

<p align="center">
  <a href="https://opencode.ai"><picture><source srcset="https://opencode.ai/brand/badges/opencode-built-with-a-dark-20.svg" media="(prefers-color-scheme: dark)"><source srcset="https://opencode.ai/brand/badges/opencode-built-with-a-light-20.svg" media="(prefers-color-scheme: light)"><img src="https://opencode.ai/brand/badges/opencode-built-with-a-light-20.svg" alt="Built with OpenCode" height="20"></picture></a>
  <img src="https://img.shields.io/badge/engine-Rune-242424?style=flat-square&labelColor=161616" alt="Engine: Rune" height="20">
  <img src="https://img.shields.io/badge/backends-Metal%20%C2%B7%20DX12%20%C2%B7%20Vulkan%20%C2%B7%20WebGPU-242424?style=flat-square&labelColor=161616" alt="Backends: Metal, DX12, Vulkan, WebGPU" height="20">
  <img src="https://img.shields.io/badge/electron-0%20MB-242424?style=flat-square&labelColor=161616" alt="Electron: 0 MB" height="20">
  <img src="https://img.shields.io/badge/tests-0-242424?style=flat-square&labelColor=161616" alt="Tests: 0" height="20">
</p>

<p align="center">
  <picture>
    <source srcset="docs/assets/app-hero-dark.png" media="(prefers-color-scheme: dark)">
    <source srcset="docs/assets/app-hero-light.png" media="(prefers-color-scheme: light)">
    <img src="docs/assets/app-hero-light.png" alt="OpenCode (Rune) session with native GPU generative UI, vector flowchart, and split diff review" width="100%">
  </picture>
</p>

## Try the `dev` build

No Electron, no DOM, no Chromium at startup. Download the latest `dev` build and point it at any running OpenCode server:

| Platform | Download (`dev`) |
| --- | --- |
| **macOS** | [Apple Silicon (`.dmg`)](https://anoma.ly/release/opencode/desktop-rune/dev/macos-arm64-dmg) · [Intel (`.dmg`)](https://anoma.ly/release/opencode/desktop-rune/dev/macos-x64-dmg) |
| **Windows** | [x64 setup](https://anoma.ly/release/opencode/desktop-rune/dev/windows-x64-setup) · [arm64 setup](https://anoma.ly/release/opencode/desktop-rune/dev/windows-arm64-setup) |
| **Linux x64** | [AppImage](https://anoma.ly/release/opencode/desktop-rune/dev/linux-x64-appimage) · [DEB](https://anoma.ly/release/opencode/desktop-rune/dev/linux-x64-deb) · [RPM](https://anoma.ly/release/opencode/desktop-rune/dev/linux-x64-rpm) |
| **Linux arm64** | [AppImage](https://anoma.ly/release/opencode/desktop-rune/dev/linux-arm64-appimage) · [DEB](https://anoma.ly/release/opencode/desktop-rune/dev/linux-arm64-deb) · [RPM](https://anoma.ly/release/opencode/desktop-rune/dev/linux-arm64-rpm) |

<sub>Set <code>chmod +x</code> on Linux AppImage files. macOS and Windows <code>dev</code> builds auto-update in the background. Want to test a pull request alongside <code>dev</code>? See <a href="docs/trying-opencode-rune.md">docs/trying-opencode-rune.md</a>. Release channels and signing: <a href="docs/releasing.md">docs/releasing.md</a>.</sub>

Start a local OpenCode server (`opencode serve --service` or open **OpenCode Desktop**), then launch **OpenCode (Rune)**. It discovers `http://127.0.0.1:4096` or `~/.config/opencode/service.json` automatically (`--server <url>` or `OPENCODE_SERVER_URL` overrides it) and never starts a server on its own.

## What's inside

<p align="center">
  <img src="docs/assets/showcase-terminal.png" alt="Side tabs, session timeline, and Ghostty-VT terminal pane" width="49%">
  <img src="docs/assets/showcase-diagram.png" alt="Native GPU vector sequence diagram in light mode" width="49%">
</p>
<p align="center">
  <img src="docs/assets/showcase-appearance.png" alt="Settings Appearance showing Windfoil GPU outline text rendering" width="49%">
  <img src="docs/assets/showcase-egg.png" alt="New-session wordmark Easter egg: 800-pin 3D GPU board and lodger" width="49%">
</p>

- **Windfoil GPU text.** Draws each glyph's exact coverage directly from its vector outline on the GPU, so text stays sharp while zooming or moving.
- **Zero Chromium at startup.** The built-in browser pane (`⌃⌘B` / `Ctrl+Alt+B`) fetches an isolated CEF sidecar (`apps/browser-engine`) lazily on demand. If you never open it, you never pay for it.
- **Real native engines.** **Ghostty-VT** (`libghostty-vt`) terminal tabs, virtualized syntax-highlighted code and split diffs (`opencode-code-view`), native GPU ```` ```ui ```` generative UI and vector diagrams (`rune-parts`), Word/Excel/PowerPoint previews via Vello (`office-preview`), local **Hex** voice dictation (`hex-client`), crash-safe SQLite with an mmap draft journal (`opencode-storage`), and 63 locales with RTL.
- **0 ms UI iteration.** Built-in `--!strict` Luau plugins (`plugins/*`) reload live in place with `--plugin-hot` without touching `rustc`.
- **Headless eyes, zero tests.** By policy, this repo has no automated tests or mocks (*"Application code is truth"*, see [AGENTS.md](AGENTS.md)). The binary renders any scene or 111-story playground view offscreen to PNG (`--snap`), dumps its live element tree (`--tree`), and replays scripted input (`--script`).
- **Weird things built right in.** Pull on the final `e` of the wordmark on a new-session tab — something lives inside it, and pulling it out pops the letters into an 800-pin 3D GPU board and Brick Breaker with synthesized sound (`features/egg`). Press `⌘⇧⌥R` (`Dev › Rewind`) to freeze the last minute of layout frames and scrub a glitch frame by frame.

## Run from source

Start an OpenCode server locally (`opencode serve --service` or **OpenCode Desktop**), then run:

```sh
cargo run -p opencode-desktop -- --live
```

For fast local iteration (skips debuginfo, Office fonts, and standalone story modules, and enables Luau `--plugin-hot` live reload):

```sh
scripts/dev-fast.sh          # macOS / Linux
pwsh scripts/dev-fast.ps1    # Windows
```

To scaffold a new Luau plugin (`plugins/<id>/`) or decoupled Rust feature crate (`features/<id>/`), run `scripts/new-feature.sh` (or `pwsh scripts/new-feature.ps1`). See [docs/fast-builds.md](docs/fast-builds.md) for profiles, `sccache`, and fast linkers, and [`crates/opencode-plugin-spec/docs/writing-a-plugin.md`](crates/opencode-plugin-spec/docs/writing-a-plugin.md) for the plugin guide.

### Other targets

| Target | Command |
| --- | --- |
| **Standalone `session-view` playground** (no server) | `cargo run -p session-view --example read_only` |
| **Local macOS `.app`** (`~/Applications/OpenCode (Rune).app`) | `apps/desktop/scripts/dev.sh` (or `--release`) |
| **Browser `wasm32`/WebGPU** (`apps/web`) | `pwsh scripts/web.ps1` |

> **Release builds:** `--profile release` is what ships — fat LTO, one codegen unit, plus the PGO profiles in `apps/desktop/pgo/` via `pwsh apps/desktop/scripts/release.ps1` (Windows x64) and `apps/desktop/scripts/build-app.sh` (Apple Silicon macOS). A plain `cargo build --release` omits the PGO profile and takes 25–60 minutes and ~10 GB RAM for one `rustc`. Use `--profile release-quick` or `--profile release-thin` for fast optimized iteration ([docs/fast-builds.md](docs/fast-builds.md)).

## Architecture

Organized by **progressive disclosure** across four tiers: start at Tier 1 and step down only when a feature needs lower-level metal. Full crate inventory, dependency graph, and wiring guide live in [ARCHITECTURE.md](ARCHITECTURE.md).

<p align="center">
  <picture>
    <source srcset="docs/assets/architecture-dark.svg" media="(prefers-color-scheme: dark)">
    <source srcset="docs/assets/architecture-light.svg" media="(prefers-color-scheme: light)">
    <img src="docs/assets/architecture-light.svg" alt="OpenCode (Rune) four-tier progressive disclosure architecture" width="100%">
  </picture>
</p>

| Tier | Path | Purpose |
| --- | --- | --- |
| **Tier 1** | `plugins/*` | Built-in `--!strict` Luau plugins (panels, dialogs, commands, server sources); hot-reloads without touching Rust |
| **Tier 2** | `features/*` | Decoupled Rust feature crates (`opencode-feature-*`) with zero cross-feature deps and non-generic `&mut dyn *Host` boundaries |
| **Tier 3** | `crates/*` | Shell kernel (`opencode-kernel`), UI foundation (`opencode-ui`), headless runtime (`opencode-runtime`), platform/storage/search/i18n/plugin-runtime libraries, and embeddable crates (`session-view`, `opencode-code-view`, `opencode-kit`, `rune-parts`, `terminal`, `browser-pane`, `office-preview`, `project-icon`, `hex-client`, `otlp-lite`) |
| **Tier 4** | `apps/*` | Host shells and packaging: `apps/desktop` (`opencode-desktop` lib + native bin), `apps/web` (`opencode-web` wasm32/WebGPU), `apps/installer` (`opencode-rune-setup`), and `apps/browser-engine` (on-demand CEF sidecar, excluded from workspace) |

<details>
<summary><strong>Embedding <code>session-view</code> in another Rune app</strong></summary>

<br>

Add git dependencies from this repository pinned to a commit rev matching your `anomalyco/rune` pin:

```toml
opencode-kit = { git = "https://github.com/anomalyco/opencode-rune", rev = "<rev>" }
session-view = { git = "https://github.com/anomalyco/opencode-rune", rev = "<rev>" }
```

Hold a `session_view::ViewState` on your app state (calling `view.setup(waker, headless)` during setup), implement `session_view::TimelineHost` (using `session_view::delegate_view_state!` for the state accessors), drive `session_view::poll(s, now, tx)` each frame, and render `session_view::read_only(s, p, id, transcript)` or `session_view::timeline_area(s, p, id, transcript, pills, header)` alongside `session_view::overlays(s, p, cx)`. See [`crates/session-view/examples/read_only.rs`](crates/session-view/examples/read_only.rs) for a complete minimal host.

</details>

## Checks

This repo has no automated tests (see [AGENTS.md](AGENTS.md)). Verify changes by running the app and the cheap pre-push guardrails (architecture layers, file size, folder shape, dialog sizes, and generated tables; rules in [ARCHITECTURE.md](ARCHITECTURE.md)):

```sh
scripts/guardrails.sh          # macOS / Linux / Git Bash
pwsh scripts/guardrails.ps1    # Windows PowerShell
```

Run `scripts/clippy.sh` separately for workspace-wide `-D warnings` checks across feature sets.
