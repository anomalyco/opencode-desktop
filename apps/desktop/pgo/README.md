# Profile-guided optimisation

`<target>.profdata` is what LLVM optimises the release build with: how often each function and
branch of the app ran in a training run of the app as it ships. `<target>.txt` says what it was
trained on (commit, Rune rev, rustc, processes, functions that ran).

- **What uses it:** `apps/desktop/scripts/release.ps1` (and through it `apps/installer/scripts/package.ps1`,
  so the release workflow) adds `-Cprofile-use` for the target's file when there is one. A plain
  `cargo build --release` doesn't: time the app with a build from `release.ps1`.
- **What it's worth (Windows x64, fat LTO, measured per frame on the same scripted typing, hover,
  scroll and palette):** 14–22 % less CPU per frame than the same build without it. The exe grows
  (hot paths inlined), and functions that never ran in training are optimised for size.
- **What the training covers:** an agent turn streaming (markdown, code, tools, a subagent, a
  question), typing in short and long drafts, hovering and switching and dragging tabs, the command
  palette, scrolling and panning, side panels, menus, popovers, the sidebar, resizing, Home,
  Settings, a German run, and a launch and quit in every part; and launches by themselves, each to
  its content against a server: the first launch of a fresh profile, later launches of a small and of
  a big profile (4,800 sessions, a 10,000-message session open), the launch after an update, and a
  launch from a new copy of the exe. Office previews and the terminal
  aren't in it yet, so they build as cold code.
- **It goes stale fast:** a profile 58 commits old (a day, with Rune bumps) gave 4-9 % less CPU per
  frame than a build without one; retrained on the same code, 23-30 %. From 20 app commits on (or
  when the training commit isn't in the clone), `release.ps1` refuses to build with it unless told
  `-StaleProfileOk` (or `OPENCODE_RUNE_STALE_PROFILE_OK=1`, which reaches it through
  `package.ps1`); in the release workflow (`OPENCODE_RUNE_BUILD_CHANNEL` set) it only warns.
- **The instrumented build needs no big machine of your own:** it is whole-program (about 16 GB of
  memory for the one LTO module), and PR CI builds it. Open a draft pull request from a branch named
  `pgo-train/<anything>`: `release.ps1` then builds the Windows x64 exe with `-Cprofile-generate`
  instead of with the profile, and the run's `build-opencode-rune-pr-windows-x64` artifact is the
  training exe (same runner, toolchain and release profile as the build that ships, so its symbol
  names are the shipped ones). Run the training against it, merge the `.profraw` files with the
  toolchain's `llvm-profdata merge --sparse`, commit the two files from a branch of `dev`, and close
  the draft. Never install that build.
- **A profile names functions by symbol, and cargo hashes the release profile (LTO, codegen units,
  debug info), the Rust version and each dependency's source revision into every symbol:** a build
  with another `lto` or `debug` setting, or with another Rune pin, gets no counts from it. Extra
  flags must go through `RUSTFLAGS`, which is not in that hash (`release.ps1` does: the profile
  itself, and the line tables of a `symbols` build).
- **After a Rune pin bump, move the counts instead of training again:**
  `node scripts/pgo-rekey.mjs --counts <this profile> --names <an instrumented exe of the new code>`
  `--out <this profile> --stamp <target>.txt --commit <sha> --rune <rev>`. It rewrites the crate hash
  inside each symbol name, reads the new build's functions from the exe file (no run), and keeps a
  function's counts when the new build has it with the same control-flow hash. On the 23 Nov bump
  (81 crate hashes) 16,287 of 16,292 functions kept theirs. Train again when its report's
  `code_changed` grows.
- **Refresh it** before a release, when the app or Rune changed since `<target>.txt`'s commit (`release.ps1`
  prints how many app commits there have been), or after a Rust toolchain bump (the file format
  follows LLVM's): `pwsh apps/desktop/scripts/pgo.ps1 -Train "<training command>"`, then commit both
  files. A stale profile is never wrong: a function whose code changed since loses its counts and
  builds as it would without a profile.
- **Apple silicon:** `build-app.sh` uses the native `aarch64-apple-darwin` profile. Its training run captured Home, session, demo, queue, settings and tab badges, in English and German. The profile-use build drew the same session capture. There is no measured Mac speed-up here. Intel macOS, Windows ARM64 and Linux do not use a profile.

<!-- pgo-train 4aebe4cc9b: an instrumented build for re-keying; never merged -->
