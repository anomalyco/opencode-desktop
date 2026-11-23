# Keyboard: official vs ours

Settings > Shortcuts read from both apps on Windows (official `dev` `23fd3da786`, ours `dev-ab9a472a2a`), command by command, on 23 Nov 2026. Official's page shows a command's first chord only; ours has the same second chords where official's source has them (`tab.new` Ctrl+N, `command.palette` Ctrl+Shift+P). WS-8 refreshes this with each keyboard sweep.

- **Result:** every command both apps list has the same chord. Nine had other ids here and took official's in rune-desktop #778 (saved keybinds carry over).
- **Lists differ only by context:** official's list is the catalog of commands it has registered, so a command a page registers (the browser pane's, the new-session page's, the bottom pane's, vertical tabs' Toggle sidebar) appears there once that page has shown. Ours lists them always.
- **Shared chord:** ours-only Dictate with Hex (Ctrl+Shift+M) and the browser pane's Toggle browser toolbar (official's Ctrl+Shift+M) share a chord; the toolbar's is scoped to the browser panel in both apps, so it wins there and Dictate everywhere else.
- **macOS:** both use `mod` (Cmd on macOS, Ctrl elsewhere) with the same per-platform exceptions: Home is Cmd+B on macOS and Alt+Home elsewhere, the browser launcher Ctrl+Cmd+B on macOS and Ctrl+Alt+B elsewhere, its hard reload Cmd+Alt+R and Ctrl+F5.
- **Menus:** the app menu's items and accelerators follow official's `DESKTOP_MENU` (`desktop-menu.ts`): New Session, Settings (Ctrl+,), New Window (Ctrl+Shift+N), Close Window (Ctrl+Shift+W), the Edit keys, zoom (Ctrl+0, Ctrl++, Ctrl+-), Back and Forward, Previous and Next Session (`apps/desktop/src/shell/menus/app_menu.rs`).

## Official's commands

| Group | Command | Official | Ours | |
| --- | --- | --- | --- | --- |
| General | `context.addSelection` Add selection to context | Ctrl+Shift+L | Ctrl+Shift+L | same |
| General | `ssh.add` Add SSH server | Unassigned | Unassigned | same |
| General | `debug.heap` Capture heap snapshots | Unassigned | Unassigned | same |
| General | `updater.check` Check for Updates… | Unassigned | Unassigned | same |
| General | `tab.closeSession` Close session tab | Ctrl+Alt+W | Ctrl+Alt+W | same |
| General | `tab.close` Close tab | Ctrl+W | Ctrl+W | same |
| General | `provider.connect` Connect an integration | Unassigned | Unassigned | same |
| General | `project.copyID` Copy Project ID | Unassigned | Unassigned | same (id was `copy-project`, #778) |
| General | `logs.export` Export logs | Unassigned | Unassigned | same |
| General | `input.focus` Focus input | Ctrl+L | Ctrl+L | same |
| General | `home.toggle` Home | Alt+Home | Alt+Home | same |
| General | `common.goBack` Navigate back | Ctrl+[ | Ctrl+[ | same |
| General | `common.goForward` Navigate forward | Ctrl+] | Ctrl+] | same |
| General | `tab.new` New session | Ctrl+T | Ctrl+T | same |
| General | `browser.open` Open browser | Ctrl+Alt+B | Ctrl+Alt+B | same |
| General | `settings.open` Open settings | Ctrl+, | Ctrl+, | same |
| General | `pairing.open` Pair device | Unassigned | Unassigned | same |
| General | `config.reload` Reload configuration | Unassigned | Unassigned | same (id was `reload`, #778) |
| General | `tab.reopenClosed` Reopen closed tab | Ctrl+Shift+T | Ctrl+Shift+T | same |
| General | `settings.search.focus` Search | Ctrl+F | Ctrl+F | same |
| General | `commands.search` Search commands | Unassigned | Unassigned | same |
| General | `command.palette` Search everything | Ctrl+K | Ctrl+K | same |
| General | `project.select` Search projects | Ctrl+Shift+O | Ctrl+Shift+O | same |
| General | `sessions.search` Search sessions | Ctrl+Shift+F | Ctrl+Shift+F | same |
| General | `debug.toggle` Toggle debug bar | Unassigned | Unassigned | same |
| General | `usage.view` View usage | Unassigned | Unassigned | same |
| Session | `session.archive` Archive session | Ctrl+Shift+S | Ctrl+Shift+S | same |
| Session | `btw.ask` Ask a side question | Unassigned | Unassigned | same |
| Session | `permissions.autoaccept` Auto-accept permissions for this project | Ctrl+Shift+A | Ctrl+Shift+A | same |
| Session | `session.compact` Compact session | Unassigned | Unassigned | same (id was `compact`, #778) |
| Session | `session.copyID` Copy Session ID | Unassigned | Unassigned | same (id was `copy-session`, #778) |
| Session | `session.delete` Delete session | Unassigned | Unassigned | same |
| Session | `session.export` Export session | Unassigned | Unassigned | same (id was `export`, #778) |
| Session | `session.search` Find... | Ctrl+F | Ctrl+F | same |
| Session | `session.fork` Fork from message | Unassigned | Unassigned | same (id was `fork`, #778) |
| Session | `session.jumpToMessage` Jump to message | Ctrl+Alt+G | Ctrl+Alt+G | same |
| Session | `session.background` Move to background | Ctrl+B | Ctrl+B | same |
| Session | `session.new` New session | Unassigned | Unassigned | same (id was `new`, #778) |
| Session | `message.next` Next message | Ctrl+Alt+] | Ctrl+Alt+] | same |
| Session | `session.next` Next session | Alt+↓ | Alt+↓ | same |
| Session | `session.next.unseen` Next unread session | Alt+Shift+↓ | Alt+Shift+↓ | same |
| Session | `review.open` Open review | Ctrl+Alt+R | Ctrl+Alt+R | same |
| Session | `message.previous` Previous message | Ctrl+Alt+[ | Ctrl+Alt+[ | same |
| Session | `session.previous` Previous session | Alt+↑ | Alt+↑ | same |
| Session | `session.previous.unseen` Previous unread session | Alt+Shift+↑ | Alt+Shift+↑ | same |
| Session | `session.redo` Redo | Unassigned | Unassigned | same (id was `redo`, #778) |
| Session | `session.rename` Rename session | Ctrl+R | Ctrl+R | same |
| Session | `review.toggle` Toggle review | Ctrl+Shift+R | Ctrl+Shift+R | same |
| Session | `details.toggle` Toggle session details | Ctrl+Shift+Y | Ctrl+Shift+Y | same |
| Session | `session.undo` Undo | Unassigned | Unassigned | same (id was `undo`, #778) |
| Navigation | `file.attach` Add files | Ctrl+U | Ctrl+U | same |
| Navigation | `file.browse` Open file | Ctrl+Alt+P | Ctrl+Alt+P | same |
| Navigation | `file.open` Open file | Ctrl+P | Ctrl+P | same |
| Model and agent | `model.choose` Choose model | Ctrl+' | Ctrl+' | same |
| Model and agent | `agent.cycle` Cycle agent | Ctrl+. | Ctrl+. | same |
| Model and agent | `agent.cycle.reverse` Cycle agent backwards | Ctrl+Shift+. | Ctrl+Shift+. | same |
| Model and agent | `model.variant.cycle` Cycle model variant | Ctrl+Shift+D | Ctrl+Shift+D | same |
| Model and agent | `mcp.toggle` Toggle MCPs | Ctrl+; | Ctrl+; | same |
| Terminal | `terminal.new` New terminal | Ctrl+Alt+T | Ctrl+Alt+T | same |
| Terminal | `terminal.toggle` Toggle terminal | Ctrl+\` | Ctrl+\` | same |
| Prompt | `prompt.mode.normal` Prompt | Unassigned | Unassigned | same |
| Prompt | `prompt.mode.shell` Shell | Unassigned | Unassigned | same |
| Prompt | `prompt.stop` Stop session | ESC | ESC | same |
| Prompt | `prompt.mode.toggle` Toggle shell mode | Ctrl+Shift+X | Ctrl+Shift+X | same |

## Listed here only

| Command | Ours | Why |
| --- | --- | --- |
| `browser.inspect` Comment on part of the browser page | Ctrl+Shift+C | the browser pane's; official registers it while a pane exists (`gui-plugins/src/browser/renderer.tsx`) |
| `text-rendering.cycle` Cycle text rendering | Unassigned | ours only: Settings > Appearance text rendering (`ours.md` 5.8) |
| `browser.find` Find in page | Ctrl+F | the browser pane's; official registers it while a pane exists (`gui-plugins/src/browser/renderer.tsx`) |
| `browser.address` Go to the browser's address bar | Ctrl+L | the browser pane's; official registers it while a pane exists (`gui-plugins/src/browser/renderer.tsx`) |
| `browser.newTab` New browser tab | Ctrl+T | the browser pane's; official registers it while a pane exists (`gui-plugins/src/browser/renderer.tsx`) |
| `browser.find.next` Next match | Ctrl+G | the browser pane's; official registers it while a pane exists (`gui-plugins/src/browser/renderer.tsx`) |
| `project.open` Open project | Ctrl+O | official registers it on the new-session page only (`new-session/location/dialog.tsx`) |
| `egg.play` Play Brick Breaker | Unassigned | ours only: the wordmark Easter egg (`ours.md` 5.8) |
| `browser.find.previous` Previous match | Ctrl+Shift+G | the browser pane's; official registers it while a pane exists (`gui-plugins/src/browser/renderer.tsx`) |
| `browser.reload` Reload browser page | F5 | the browser pane's; official registers it while a pane exists (`gui-plugins/src/browser/renderer.tsx`) |
| `browser.reloadPage` Reload browser page | Ctrl+R | the browser pane's; official registers it while a pane exists (`gui-plugins/src/browser/renderer.tsx`) |
| `browser.hardReload` Reload without saved files | Ctrl+F5 | the browser pane's; official registers it while a pane exists (`gui-plugins/src/browser/renderer.tsx`) |
| `reopen-window` Reopen closed window | Unassigned | ours only: reopen the last closed window |
| `bottom.toggle` Toggle bottom panel | Ctrl+J | official registers it while the bottom pane has tabs (`use-session-commands.tsx`) |
| `browser.toggleToolbar` Toggle browser toolbar | Ctrl+Shift+M | the browser pane's; official registers it while a pane exists (`gui-plugins/src/browser/renderer.tsx`) |
| `sidebar.toggle` Toggle sidebar | Ctrl+Shift+B | official registers it in vertical tabs only (`shell.tsx`) |
| `session.question.back` Back | Alt+← | the question dock's, while it shows |
| `session.branch.choose` Choose base branch | Ctrl+Alt+B | official registers it on the new-session page only |
| `session.location.select` Choose worktree | Ctrl+Alt+O | official registers it on the new-session page only |
| `session.location.cycle` Cycle session location | Ctrl+Alt+L | official registers it on the new-session page only |
| `prompt.dictate` Dictate with Hex | Ctrl+Shift+M | ours only: Hex dictation (`ours.md` 5.8) |
