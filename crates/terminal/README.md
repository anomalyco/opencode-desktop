# `terminal`

Embeddable Ghostty-VT terminal pane for Rune applications.

Streams PTY input and output over OpenCode `/api/pty` WebSockets, feeds bytes through `libghostty-vt` (fetched or built by `build.rs`), and renders cells, selection, scrollback, and cursor state in Rune.
