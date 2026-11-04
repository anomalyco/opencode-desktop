# `browser-pane`

Embeddable Chromium browser pane for Rune applications.

Talks over a local Unix socket to the out-of-process `OpenCode Browser Engine.app` CEF sidecar (`apps/browser-engine`), downloading and SHA-256-verifying the signed sidecar bundle from `https://anoma.ly/release` on first use (`installer.rs`) and streaming accelerated frames and input events into a Rune element.
