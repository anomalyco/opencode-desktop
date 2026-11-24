# Ported rows, run next to official

A ledger row is "have it" when its code is merged. This table says whether the row was also **run**: the same
case in a CI-built copy of ours and in the official app, side by side.

- **same:** both apps do the same.
- **fixed:** the run showed a difference and a follow-up PR removed it.
- **differs (owner's call):** ours does it differently or does more, on purpose; the owner decides.
- **ours only:** run in ours; the official side is not run or cannot be made in the harness.
- **not run:** nobody has run it yet.

| Row | Official commit | Ours PR | Run | Result | Notes |
| --- | --- | --- | --- | --- | --- |
| UP-176 | `9b1cdf0167` (#65707) | #822 | 23 Nov | same | The comment button is in the browser bar; the toolbar toggle is in Options > More tools and is disabled on a blank tab in both. |
| UP-177 | `b705c446f0` (#65703) | #823 | 24 Nov | same | A write of a 60-line file: the open file row shows all 60 lines in both (ours 1,416 px from first to last), no fold, no Show more. |
| UP-178 | `18be128acc` (#65690) | #824 | 23 Nov | same (caret); rings not run | The caret keeps its place when the composer gets focus again without a press (Tab and back, Ctrl+L, Home and back). The missing focus rings are not run. |
| UP-179 | `d84e1d7bb3` (#65696) | #825 | 24 Nov | same | A Code Mode script with 15 calls: the latest 10 and "+5 more" in both; a press shows all 15 and removes the button (ours leaves focus on the first call). |
| (round 21) | focus moves | #828 | 23 Nov | ours only | Toggle browser toolbar (Ctrl+Shift+M) leaves focus on Options with the ring, from the bar and from inside the page. |
| UP-180 | `b3f233c64c` (#65689) | #826, #845 | 23 Nov | differs: official is faulty | Ours links `~/notes/todo.md` and opens it, in the home of the server the window is on: on a second server it opens that server's file, also when this machine's home has a file of the same name (#845; before #845 the path was not a link there, or was refused as this machine's file). Official never links a `~` path: its store holds no home folder for the server. Draft fix: anomalyco/opencode#65773. |
| UP-182 | `512d2f33a8` (#65693) | none needed | | not run | The spinner's 90 fps picture; ours animates on the display clock. |
| UP-184 | `2720b4b20c` (#64395) | #833, #835 | 23 Nov | same | A chord that clashes gives the toast "Shortcut already in use" / "Ctrl+Alt+K is already assigned to Open settings." in both; a chord on a plugin command records and runs in ours. |
| UP-185 | `45aca07b15` (#65720) | #838 | 23 Nov | same | The browser attaches on a server that lists no plugin and refuses attach for its first seconds. Official asks the plugin list every 110 ms and attaches once; ours asks, pauses 2.3 s once, then asks every 117 ms and attaches. |
| UP-186 | `331010a1ef` (#65740) | #847 | 23 Nov | same | Settings > Providers reads the same in both, word for word: Popular lists OpenCode (Recommended), github-copilot, openai; no OpenCode Go; no note on OpenCode. |
| UP-187 | `7edab789f7` (#65749) | #857 | 23-24 Nov | same; palette row ours only | The model list has no speed rows and another model and back restores the variant and Fast, in both (UPSTREAM-d's run). Ours' palette lists "Cycle model speed". Speed kept across a restart and the narrow window are not run. |
| UP-189 | `d284e7ce17` (#65757) | #860 | 24 Nov | same | The chosen layout card's check is `#3b5cf6` in both. |
| UP-190 | `7cfd868443` (#65758) | #860 | 24 Nov | same | The timeline's and Home's scroll areas are no Tab stop in both: a click focuses the timeline without a ring; Tab and Shift+Tab pass the neighbours (Copy response, More actions; Home: New session, Settings, Help, the search field). |
| UP-192 | `2bf2468018` (#65783) | #872 | 24 Nov | same | The file toolbar's split button: each half is its own stop (20x28 and 28x28 in both) and its ring is round on the outer side, square on the inner side. |
| UP-193 | `16ba706544` (#65785) | #881 | 24 Nov | differs (owner's call, asked) | Official: the file viewer is no Tab stop; a click focuses it without a ring and Down scrolls. Ours: a click focuses the clicked line without a ring, Down moves a line cursor shown by a fill, and the viewer is a Tab stop. No ring on the focused terminal in both. |
| UP-194 | `d1f98703ab` (#65800) | #882 | 24 Nov | fixed | Layout cards are 120x78, radius 8, 10 apart in both. The check sat 7 pt outside the card in ours, 6 px in official: now 6 (this table's PR; the after-run is owed). Neither app shows navigation cards on Experimental. |
| (no row) | `fitMenuToOptions`, `menu.css` | #883 | 24 Nov | same | The agent menu is 92x60 and the variant menu 160x116 in both. |
| (no row) | `historyClearable` in `gui-plugins/browser/model.ts` | #846 | 23 Nov | same | Clear history is enabled on a blank tab in both. Official's failure toast ("Couldn't clear the history. Try again.") shows in the harness; ours clears there, so its failure toast is not run. |
| (no row) | `file/renderer.tsx` `open` | #855 | 23 Nov | same; two points differ (owner's call) | A file outside the session's folder opens in a tab in both: text, picture, a 4.2 MB file, a `file:///` link; a missing file and a UNC path in code are not links; an HTML file gives the same toast. Ours also on a remote server. Differs: a UNC markdown link is refused in ours (official opens a tab named after it), and ours' tab menu has no "Open in" (official's has). |

Not run in either app: UP-178's rings on the timeline and Home list, UP-182, the /btw unread dot (#897).

## How the runs are made

- Ours: the PR's CI artifact (`build-opencode-rune-pr-windows-x64`, checked against `SHA256SUMS`), one headless
  process per case with a sealed scratch profile, a script of taps and keys, and the element tree read at the end.
- Official: the app built from official `dev` at the commit named in the run notes, driven over CDP on a hidden
  desktop.
- Both talk to the same private simulated server and the same seeded sessions. A second server and a server with a
  home folder are a proxy in front of it.
