# `browser-pane`

Embeddable Chromium browser pane for Rune applications.

Talks over a local Unix socket to the out-of-process `OpenCode Browser Engine` CEF sidecar (`apps/browser-engine`) and streams accelerated frames and input events into a Rune element. The engine isn't part of the app: the first page fetches it from the build's source and checks it against the build's trust rule before anything of it runs (`source.rs`: `SOURCE` and `TRUST`, set with `OPENCODE_BROWSER_ENGINE_SOURCE` and `OPENCODE_BROWSER_ENGINE_TRUST` when the app is built), unpacking it as it downloads (`download.rs`, `unpack.rs`, `trust.rs`). A newer release installs beside a running engine for its next start.

On Windows a development build reads `http://127.0.0.1:47963/index.json`: build the engine with `pwsh apps/browser-engine/scripts/package-windows.ps1` and serve it with `bun apps/browser-engine/scripts/serve.ts`. `OPENCODE_BROWSER_ENGINE_BIN` or `OPENCODE_BROWSER_ENGINE_APP` runs an engine from anywhere instead.

On Linux x86_64, the installer reads `release.targets.linux-x64`. Its `.tar.zst` contains a ustar archive rooted at `OpenCode Browser Engine/`, with `opencode-browser-engine`, `opencode-browser-engine-helper` and CEF's resources together. Both programs must have execute permission in the archive. Until a release source and SHA-256 pin are configured, use `OPENCODE_BROWSER_ENGINE_BIN` or point a debug build at the `file:` URL of `package-linux.sh`'s `index.json` with `OPENCODE_BROWSER_ENGINE_SOURCE` and `OPENCODE_BROWSER_ENGINE_TRUST=local`.

To run from a Linux checkout (the pinned CEF distribution is downloaded on the first build):

```sh
cargo build --locked --release --manifest-path apps/browser-engine/Cargo.toml
OPENCODE_BROWSER_ENGINE_BIN="$PWD/apps/browser-engine/target/release/opencode-browser-engine" cargo run -p opencode-desktop
```

Press Ctrl+Alt+B and enter a URL. Linux CEF uses headless Ozone and sends BGRA software paints over the socket. The sidecar inherits the existing `no_sandbox` setting: pages and renderer processes run with the desktop user's privileges.
