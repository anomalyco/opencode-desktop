# Built-in plugins and their own redraws

A built-in moves to its own redraws (`redraw.rs`, `LIVE`) when the states it draws have been driven live, in a debug build, with `OPENCODE_PLUGIN_REDRAW=implicit` against its own redraws on the same events, and both drew byte-identical frames and trees, changing on the same frame. A code check alone doesn't move it. Keep this table and `LIVE` in step.

| Built-in | Redraw | Driven live | Still to drive |
|---|---|---|---|
| context | live | header cost and tokens on `session.usage.updated` | export, raw message expand |
| debug | live | bar shown from the palette, its timing cells | heap capture, focus mode |
| details | live | status popover opened | list loading/failed, MCP toggle, sign-in |
| file | live | file filter, opening a file (tab count, title) | read failure, rename |
| goal | live | set, pause, resume, complete, clear; duplicate and other-session answers draw nothing | command pending and failure |
| pairing | live | keep-screen switch, pair dialog opened | code refresh and expiry, copy |
| review | live | diff style | count change on a new diff, file selection |
| rune-ui | live | streaming code block | invalid input and restore |
| terminal | live | title (OSC 2), bell flash, output | rename, find bar |
| browser | legacy | address field focus and edit | a real page's loading and title, new tab, site popover |
| btw | legacy | none | ask, answer, retry |
| microsoft-office | legacy | none | document read, sheet switch, failure |
| sources | legacy | none | a turn's searches, expand, favicon |
| ssh | legacy | none | connect, authentication prompt, failure |
| updater | legacy | none | checking, up to date, downloading, ready, download required, error |
| wsl | legacy | none | runtime probe, distro list, install and add server |

The "live" rows were driven on Windows (a debug build, headless) and on macOS (the same cases, headless on Metal). Both gave byte-identical frames and trees between the legacy redraw and the plugin's own, changing on the same frame on both platforms. Nothing has run on Linux. The redraw logic is shared code, so what could differ there is when the platform delivers the events a plugin redraws on.
