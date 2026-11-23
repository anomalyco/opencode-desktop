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
| UP-177 | `b705c446f0` (#65703) | #823 | | not run | An open file row shows its whole diff (no 420 px fold). |
| UP-178 | `18be128acc` (#65690) | #824 | 23 Nov | same (caret); rings not run | The caret keeps its place when the composer gets focus again without a press (Tab and back, Ctrl+L, Home and back). The missing focus rings are not run. |
| UP-179 | `d84e1d7bb3` (#65696) | #825 | | not run | An open Code Mode script lists its latest 10 calls with a "+N more" button. |
| (round 21) | focus moves | #828 | 23 Nov | ours only | Toggle browser toolbar (Ctrl+Shift+M) leaves focus on Options with the ring, from the bar and from inside the page. |
| UP-180 | `b3f233c64c` (#65689) | #826, #845 | 23 Nov | differs: official is faulty | Ours links `~/notes/todo.md` and opens it, in the home of the server the window is on: on a second server it opens that server's file, also when this machine's home has a file of the same name (#845; before #845 the path was not a link there, or was refused as this machine's file). Official never links a `~` path: its store holds no home folder for the server. Draft fix: anomalyco/opencode#65773. |
| UP-182 | `512d2f33a8` (#65693) | none needed | | not run | The spinner's 90 fps picture; ours animates on the display clock. |
| UP-184 | `2720b4b20c` (#64395) | #833, #835 | 23 Nov | same | A chord that clashes gives the toast "Shortcut already in use" / "Ctrl+Alt+K is already assigned to Open settings." in both; a chord on a plugin command records and runs in ours. |
| UP-185 | `45aca07b15` (#65720) | #838 | 23 Nov | same | The browser attaches on a server that lists no plugin and refuses attach for its first seconds. Official asks the plugin list every 110 ms and attaches once; ours asks, pauses 2.3 s once, then asks every 117 ms and attaches. |
| UP-186 | `331010a1ef` (#65740) | #847 | 23 Nov | same | Settings > Providers reads the same in both, word for word: Popular lists OpenCode (Recommended), github-copilot, openai; no OpenCode Go; no note on OpenCode. |
| (no row) | `historyClearable` in `gui-plugins/browser/model.ts` | #846 | 23 Nov | same | Clear history is enabled on a blank tab in both. Official's failure toast ("Couldn't clear the history. Try again.") shows in the harness; ours clears there, so its failure toast is not run. |
| (no row) | `file/renderer.tsx` `open` | #855 | 23 Nov | same; two points differ (owner's call) | A file outside the session's folder opens in a tab in both: text, picture, a 4.2 MB file, a `file:///` link; a missing file and a UNC path in code are not links; an HTML file gives the same toast. Ours also on a remote server. Differs: a UNC markdown link is refused in ours (official opens a tab named after it), and ours' tab menu has no "Open in" (official's has). |

## How the runs are made

- Ours: the PR's CI artifact (`build-opencode-rune-pr-windows-x64`, checked against `SHA256SUMS`), one headless
  process per case with a sealed scratch profile, a script of taps and keys, and the element tree read at the end.
- Official: the app built from official `dev` at the commit named in the run notes, driven over CDP on a hidden
  desktop.
- Both talk to the same private simulated server and the same seeded sessions. A second server and a server with a
  home folder are a proxy in front of it.
