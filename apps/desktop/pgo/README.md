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
  Settings, a German run, and a launch and quit in every part. Office previews and the terminal
  aren't in it yet, so they build as cold code.
- **It goes stale fast:** a profile 58 commits old (a day, with Rune bumps) gave 4-9 % less CPU per
  frame than a build without one; retrained on the same code, 23-30 %. From 20 app commits on (or
  when the training commit isn't in the clone), `release.ps1` refuses to build with it unless told
  `-StaleProfileOk` (or `OPENCODE_RUNE_STALE_PROFILE_OK=1`, which reaches it through
  `package.ps1`); in the release workflow (`OPENCODE_RUNE_BUILD_CHANNEL` set) it only warns.
- **Refresh it** before a release, when the app or Rune changed since `<target>.txt`'s commit (`release.ps1`
  prints how many app commits there have been), or after a Rust toolchain bump (the file format
  follows LLVM's): `pwsh apps/desktop/scripts/pgo.ps1 -Train "<training command>"`, then commit both
  files. A stale profile is never wrong: a function whose code changed since loses its counts and
  builds as it would without a profile.
- **Apple silicon:** `build-app.sh` uses the native `aarch64-apple-darwin` profile. Its training run captured Home, session, demo, queue, settings and tab badges, in English and German. The profile-use build drew the same session capture. There is no measured Mac speed-up here. Intel macOS, Windows ARM64 and Linux do not use a profile.
