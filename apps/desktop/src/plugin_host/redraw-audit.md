# Built-in plugins and their own redraws

A built-in moves to its own redraws (`redraw.rs`, `LIVE`) when the states it draws have been driven live, in a debug build, with `OPENCODE_PLUGIN_REDRAW=implicit` against its own redraws on the same events, and both drew the same frames and trees, changing on the same frame. A case counts only if each run shows more than one state, and at least one of a plugin's cases is a change that lands on a frame with no input behind it (a service event, a process or server answering, a timer): input asks for a frame whichever redraw rule is on, so a case driven only by input can't tell the two apart. A code check alone doesn't move it. Keep this table and `LIVE` in step.

| Built-in | Redraw | Driven live | Still to drive |
|---|---|---|---|
| context | live | header cost and tokens on `session.usage.updated` (a service event, no input) | export, raw message expand |
| debug | legacy | input only: the bar shown from the palette (its slide moves the layout for 13 frames; nothing the plugin draws changes on its own) | its timing cells updating, heap capture, focus mode |
| details | legacy | input only: status popover opened (it lands on the tap's release) | list loading/failed, MCP toggle, sign-in |
| file | legacy | input only: file filter, opening a file (the contents land on the tap's release) | a slow read landing, read failure, rename |
| goal | live | set, pause, resume, complete, clear on `rpc.opencode.goal.changed` (a service event, no input); duplicate and other-session answers draw nothing | command pending and failure |
| pairing | legacy | input only: keep-screen switch (its thumb slides after the release), pair dialog opened | the power answer, code loading, refresh and expiry, copy |
| review | legacy | input only: diff style (its rows change on the release and the frame after) | count change on a new diff, file selection |
| rune-ui | live | streaming code block (the stream grows every frame, no input) | invalid input and restore |
| terminal | live | title (OSC 2) and bell from the terminal's output a frame after the typing; the bell flash ending on its own 140 ms timer; output | rename, find bar |
| browser | legacy | address field focus and edit; site popover (loading, then failed without an engine) | a real page's loading and title, a new tab |
| btw | legacy | ask (tab opens, running, failed without a model); Retry (running, failed) | an answer landing, which needs a model the fixture doesn't have |
| microsoft-office | legacy | none | document read, sheet switch, failure |
| sources | live | against a loopback server with simulated web search and fetch, on its "Search the web" turn (headless, Sources on): the chip opening the turn's tab, its searches landing a frame later from the server's answer (no input), and expanding its search; 30 frames each, byte-identical with either redraw | favicons fetched from sites, the experiment turned off |
| ssh | live | live window, a stand-in `ssh.exe` beside the exe (the plugin runs `ssh.exe` by name): "Connecting over SSH.", the password prompt arriving on its own 1.5 s after Enter, the service registering and the dialog closing 1.2 s after the answer; and "Could not establish the SSH connection" arriving on its own 2.5 s after Enter; each element's values in the same order with either redraw | a reconnect cover, a host-key confirmation, a dropped tunnel |
| updater | live | release-notes switch (headless); live window against a local feed offering a newer version: the service went Idle, Checking, Downloading, Ready, and the window went from no toast to the "Update Ready" toast with Restart (no input), the same states in the same order with either redraw | up to date and error on screen, download required, installing |
| wsl | live | live window, a stand-in `wsl.exe` through `--wsl-exe`: the distro list arriving 2.5 s late, then "Checking." and ready from the distro probe; the list failing half-way; a catalog install finishing 3 s after the click and the new distro listed; each element's values in the same order with either redraw | add server and its server starting, install of WSL itself |

The "live" rows were driven on Windows (a debug build, headless) and on macOS (the same cases, headless on Metal, except rune-ui, sources, ssh, updater and wsl: Windows only). Both gave byte-identical frames and trees between the legacy redraw and the plugin's own, changing on the same frame on both platforms. Nothing has run on Linux. The redraw logic is shared code, so what could differ there is when the platform delivers the events a plugin redraws on.
