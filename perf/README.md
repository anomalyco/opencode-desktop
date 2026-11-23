# perf: the launch bench

`launch.bench.ts` measures the app's launch with [strobe](https://github.com/anomalyco/strobe) from the process's spawn. The app profile it launches is `opencode-rune` in `strobe.json` at the repository's root.

## Run it

```sh
# Is build B's launch faster than build A's? One series: a case and a target.
strobe bench perf/launch.bench.ts --base exe:<A>/opencode-desktop.exe --head exe:<B>/opencode-desktop.exe --filter warm --target small

# Every case once on one build, no verdict (does it start, do the milestones arrive).
strobe bench perf/launch.bench.ts --head exe:<B>/opencode-desktop.exe --check

# Where the time goes: a recording with stacks, then its hotspots and threads.
strobe record --bench perf/launch.bench.ts#warm --head exe:<B>/opencode-desktop.exe --target small
strobe query <rec> hotspots --thread main
strobe query <rec> threads --timeline
strobe open <rec>
```

`--why always` on a `bench` adds an attribution block (stacks) to each series.

| Case | The launch |
|---|---|
| `first` | A fresh profile, never run before (target `none`). |
| `first-desktop` | A fresh profile on a machine where OpenCode Desktop (the official app) left its database (target `none`): the first launch takes Desktop's windows, tabs and drafts. `desktop-profile.ts` makes the database (`OPENCODE_LAUNCH_DESKTOP_MB`, 64 by default). |
| `first-desktop-running` | The same while Desktop runs: its database has a WAL with frames, so the database and the WAL are read into memory (the WAL's committed frames applied) instead of the few pages a closed Desktop's file gives. |
| `first-new-build` | A profile an earlier run made, the exe a new file, none of the exe's own caches: the launch after an update. `OPENCODE_LAUNCH_PROFILE_BY=<older exe>` makes the profile with another build. |
| `cold` | The exe's pages not in memory, GPU shader cache cold. |
| `warm` | A later launch, everything cached. |

| Target | The profile and its server |
|---|---|
| `none` | No profile. |
| `small` | 5 projects, 12 sessions, a 10-message session open. |
| `big` | 40 projects, 4,800 sessions, a 10,000-message session open. |

The primary is `launch.first_frame` (the app's `frame.first.presented`). The other milestones are the app's own launch marks, printed by `--launch-marks` with each mark's time to the microsecond: `launch.main`, `launch.window`, `launch.input_ready`, `launch.server`, `launch.content_live`. A launch ends at `content_live`. A build from before 24 Nov 2026 prints no `main.entered` or `window.created`: set `OPENCODE_LAUNCH_EARLY=0` for it.

## What it needs

- **An exe per variant.** The bench never builds. Use CI's artifact (`build-opencode-rune-pr-windows-x64` of a pull request's run, the signed zip of a release) or a profiling build.
- **A build with symbols for stacks.** CI's artifacts carry no PDB or dSYM, so their stacks do not resolve. A profiling build is `cargo build --profile perf -p opencode-desktop` (the `perf` profile in `Cargo.toml`: optimised as `release-quick`, 16 codegen units and no cross-crate LTO, with line tables kept), which leaves `opencode-desktop.pdb` beside the exe on Windows. Read strobe's `! Coverage` line and its two resolved shares before trusting a hotspot table, and say which build a recording used.
- **A fixture server.** The app is started with `--server`; the bench does not start OpenCode's own. Either servers already running (`OPENCODE_LAUNCH_SERVER_SMALL`, `OPENCODE_LAUNCH_SERVER_BIG`: their URLs) or `OPENCODE_LAUNCH_FIXTURE=<script>`, started as `bun <script> --port <p> --projects <n> --sessions <n> --messages <n>` once per size (ports `OPENCODE_LAUNCH_PORT_SMALL` 5871 and `OPENCODE_LAUNCH_PORT_BIG` 5872). The fixture is not in this repository yet.
- **`cold` and `first-new-build` replace the variant's exe file** with a new file of the same bytes at the same path before each launch (an unbuffered copy): point them at a copy of a build, not at the only one.
- **`warm` and `first` start with the exe's pages in memory.** Before each of their launches the bench touches every page of the exe's image (Windows), as a launch a moment earlier leaves them. Without it a machine short of memory drops the pages between launches and a "warm" launch reads them again before `main` (`main` at 85-1,873 ms instead of 20 in 5 of 24 launches; with it 2 of 24). `OPENCODE_LAUNCH_WARM_EXE=0` turns it off.
- **A window nobody can see gives no `launch.painted`.** On a hidden desktop strobe reads no frame of the window (`NoPresentSource`); the first-frame clock is the app's own `frame.first.presented` mark either way.
- Profiles are made once per exe and size by two untimed launches of that exe and kept in `OPENCODE_LAUNCH_SCRATCH` (the system's temporary folder by default).

## content.bench.ts: from the window to content

A later launch that restores the session the window had open, against **OpenCode's own server** holding a real-size history (not the fixture): the app is given no `--server` and finds the server from `<state>/opencode/service.json`, as an installed copy does.

```sh
OPENCODE_BENCH_SERVER=http://127.0.0.1:<port> OPENCODE_BENCH_PASSWORD=<password> OPENCODE_BENCH_SESSION=<session id> \
  strobe bench perf/content.bench.ts --base exe:<A>/opencode-desktop.exe --head exe:<B>/opencode-desktop.exe --filter live
```

| Case | Ends at |
|---|---|
| `live` | `content.live`: the server answered and the open session's rows show. |
| `session` | `session.read.received`: the open session's newest page came from the server. |
| `settled` | The window's last change (its pixels: needs a desktop a person could see). |

Milestones: `launch.first_frame`, `launch.input_ready`, `launch.rows` (the first frame with the session's messages: the saved copy's), `launch.server`, `launch.content_live`, `launch.session_read`. No case ends at a row of the timeline: the saved copy puts the session's last turns in the first frame, so strobe finds them in its first read of the window and refuses to time them.

- The server is yours to start: any `opencode serve` whose port is not 4096 (the app's default address, which a launch falls back to when the registered server is away: the bench refuses to run while something listens there).
- `OPENCODE_BENCH_ENGINE=<folder>`: an installed browser engine (`<state>/co.anomaly.opencode/browser-engine/<version>` of a profile that has one), linked into each launch's home. Without it every launch starts the engine's download once its content is up.
- Layout shifts: start the app with `--launch-trace <file> --launch-trace-shifts` and read `shifts` in its line (frames in which an element on screen moved, how many moved, and the first of them by name). It walks every frame's elements, so it is off in a timed launch.
## Not covered yet

Several windows, the browser pane restored, an update pending, a launch from a folder a virus scanner reads.

## macOS, started as the Dock starts it

strobe spawns its target as a child process. The Dock and Finder start an app through Launch Services, which costs about 20,000 to 55,000 µs more on an M4 MacBook Air (launchd starts the process, and AppKit waits for the open-application Apple event). `perf/macos-open.ts` is one such launch (`open -n -W -a <.app>`) for `strobe run`: it prints the app's own launch marks as `strobe:metric` lines, in µs from the kernel's start time of the process.

```sh
strobe run --base "bun perf/macos-open.ts /path/Old.app" --head "bun perf/macos-open.ts /path/New.app" \
  --primary metric.first_frame --metrics metric.input_ready,metric.content_live,metric.main,metric.resumed,metric.window
```

- Profiles `small` (the default), `first` and `cold` (a copy of the bundle at a new path); a fourth argument `child` starts the bundle's exe directly.
- The fixture server must already answer at `OPENCODE_LAUNCH_SERVER_SMALL`: a run that started one and left it behind is refused by strobe.
- The bundle must be signed (ad hoc is enough). A file added inside a signed bundle makes every later start from that path hang before `main`.
- For stacks, record the child launch (`strobe record --bench perf/launch.bench.ts#warm --head exe:<.app>/Contents/MacOS/opencode-desktop`) with the dSYM beside the bundle. macOS samples are untimed snapshots: rows are counts, not ms, and the sampler inflates `dlopen`.
