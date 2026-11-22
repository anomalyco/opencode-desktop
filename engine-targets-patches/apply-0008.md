Unblocks `verify-engine-macos (x64, macos-26-intel)`, which fails on every run since #622 added the live probe. Without this, `publish-engine` is skipped and the release publishes nothing.

Why (run `44984335877` on `d034ad3a84`, job `137094165304`, printed by `0007`'s failure report):
- The signed x64 engine printed `listening`, sent `ready` (1.45 s) and loaded the probe page (2.09 s).
- In the GPU process, ANGLE failed to start any EGL display (`Initialization of all (1) EGL display types failed`). Each try ended with `Exiting GPU process due to errors during initialization` (exit 5).
- After 3 crashes: `FATAL: GPU process isn't usable. Goodbye.` The engine exited (`-5`) before the probe listed renderers, which gave `no renderer subprocess found`. The same failure, without the report, happened in run `44980898474` (job `137082687621`).
- The macOS engine bundle deliberately ships no SwiftShader (`apps/browser-engine/scripts/package-release.sh:205-206`, since #172/#222: "macOS uses Metal"). On a runner without a usable Metal/GL device, Chromium has no fallback, so the engine cannot stay up there. Real Intel Macs have Metal. The arm64 job passes, and the same probe passes on `air` (`proof\engine-targets\prx\mac-probe-0007-31da2894.txt`).
- Not caused by #622 or #683: neither touches packaging or GPU switches. Revision 8 had the same bundle, but no live probe ran on x64 before #622.
- Not proved: whether the GPU sandbox changes anything on that VM. A sandbox cannot create a GPU, and the error is that no EGL display is available at all.

Change (`.github/workflows/release.yml` only):
- **arm64:** `verify-engine-macos` runs the live probe (`--require-sandbox`), unchanged.
- **x64:** keeps every static check (stapler, `spctl`, `codesign` with the team requirement, arch, version, `libcef_sandbox.dylib` present) and writes a summary line instead of the live probe.

```sh
git fetch origin && git checkout -b ci/engine-probe-macos-arm64 origin/dev
git am 0008-ci-release-run-the-live-macOS-engine-probe-on-arm64-.patch
git diff --stat origin/dev..HEAD && git rev-parse HEAD:.github
git push -u origin ci/engine-probe-macos-arm64
```

- **Patch SHA-256:** `c7afbe3222b9b46e26eeb6d8bcdb2e64162e957a2ac17f090014b27052c8f745` (`2790` B)
- **Expected diffstat:** `1 file changed, 10 insertions(+), 2 deletions(-)`
- **Expected `HEAD:.github` (on `ef665aef7`):** `c05975cbd05803b234d65605febcf60722f62219`
- **Follow-up (not for today):** a live x64 check needs either a runner with a GPU or SwiftShader in the macOS bundle (an engine packaging change; about 41 MiB).
