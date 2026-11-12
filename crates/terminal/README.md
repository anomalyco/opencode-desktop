# `terminal`

Embeddable Ghostty-VT terminal pane for Rune applications.

Streams PTY input and output over OpenCode `/api/pty` WebSockets, feeds bytes through `libghostty-vt` (fetched or built by `build.rs`), and renders cells, selection, scrollback, and cursor state in Rune.

In the browser build (`apps/web`) `build.rs` builds `libghostty-vt` for `wasm32-freestanding` (no SIMD: its SIMD paths are C++), which links into the page's module. A persistent PTY attaches over the page's own `WebSocket` on a task of its event loop (`persistent_pty_web.rs`), with the same worker as natively.
