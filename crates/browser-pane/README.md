# `browser-pane`

Embeddable Chromium browser pane for Rune applications.

Talks over a local Unix socket to the out-of-process `OpenCode Browser Engine` CEF sidecar (`apps/browser-engine`) and streams accelerated frames and input events into a Rune element. The engine isn't part of the app: the first page fetches it from the build's source and checks it against the build's trust rule before anything of it runs (`source.rs`: `SOURCE` and `TRUST`, set with `OPENCODE_BROWSER_ENGINE_SOURCE` and `OPENCODE_BROWSER_ENGINE_TRUST` when the app is built), unpacking it as it downloads (`download.rs`, `unpack.rs`, `trust.rs`). A newer release installs beside a running engine for its next start.

On Windows a development build reads `http://127.0.0.1:47963/index.json`: build the engine with `pwsh apps/browser-engine/scripts/package-windows.ps1` and serve it with `bun apps/browser-engine/scripts/serve.ts`. `OPENCODE_BROWSER_ENGINE_BIN` or `OPENCODE_BROWSER_ENGINE_APP` runs an engine from anywhere instead.
