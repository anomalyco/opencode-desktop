# `browser-pane`

Embeddable Chromium browser pane for Rune applications.

Talks over a local Unix socket to the out-of-process `OpenCode Browser Engine` CEF sidecar (`apps/browser-engine`) and streams accelerated frames and input events into a Rune element. The engine isn't part of the app: the first page fetches it from the build's source and checks it against the build's trust rule before anything of it runs (`source.rs`: `SOURCE` and `TRUST`, set with `OPENCODE_BROWSER_ENGINE_SOURCE` and `OPENCODE_BROWSER_ENGINE_TRUST` when the app is built), unpacking it as it downloads (`download.rs`, `unpack.rs`, `trust.rs`). A newer release installs beside a running engine for its next start.

On Windows a development build reads `http://127.0.0.1:47981/index.json`: build the engine with `pwsh apps/browser-engine/scripts/package-windows.ps1` and serve it with `bun apps/browser-engine/scripts/serve.ts`. A release build never reads it: without a source it says the browser isn't published. `OPENCODE_BROWSER_ENGINE_BIN` or `OPENCODE_BROWSER_ENGINE_APP` runs an engine from anywhere instead.

Each install records the rule that accepted it (`<version>/trust`), and an engine starts only when the build's rule accepts that record, so a release build never runs an engine a development build took from the stand-in.

A Windows release of the engine with a publisher rule: `package-windows.ps1 -Arch <arch> -StageOnly` builds and stages `OpenCode Browser Engine/`; sign every `.exe` and `.dll` in it with the app's Authenticode identity; `package-windows.ps1 -Arch <arch> -ArchiveOnly <stage> -Out <dir>` archives it and writes its `index.json` entry. Build the app with `OPENCODE_BROWSER_ENGINE_SOURCE=<the published index>` and `OPENCODE_BROWSER_ENGINE_TRUST=publisher:<the signer's name>`.

On Linux x86_64, the installer reads `release.targets.linux-x64`. Its `.tar.zst` contains a ustar archive rooted at `OpenCode Browser Engine/`, with `opencode-browser-engine`, `opencode-browser-engine-helper` and CEF's resources together. Both programs must have execute permission in the archive. Until a release source and SHA-256 pin are configured, use `OPENCODE_BROWSER_ENGINE_BIN` or point a debug build at the `file:` URL of `package-linux.sh`'s `index.json` with `OPENCODE_BROWSER_ENGINE_SOURCE` and `OPENCODE_BROWSER_ENGINE_TRUST=local`.

To run from a Linux checkout (the pinned CEF distribution is downloaded on the first build):

```sh
cargo build --locked --release --manifest-path apps/browser-engine/Cargo.toml
OPENCODE_BROWSER_ENGINE_BIN="$PWD/apps/browser-engine/target/release/opencode-browser-engine" cargo run -p opencode-desktop
```

Press Ctrl+Alt+B and enter a URL. Linux CEF uses headless Ozone and sends BGRA software paints over the socket. The sidecar inherits the existing `no_sandbox` setting: pages and renderer processes run with the desktop user's privileges.
