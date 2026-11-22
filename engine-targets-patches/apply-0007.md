Merge this together with rune-desktop #683 (`fix/linux-sandbox-off`, head `9e2abfa74b709316da4d9da55164ea47f94183d5`). Together they make `build-engine-linux` (x64 + arm64) prove "the engine starts and loads a page" again. #683 alone still fails the Linux probe, because the old script looks for `--type=renderer` and requires `Seccomp: 2`. This patch alone keeps failing on today's `dev` (`b0eb52c004`), but the job log then shows why.

Why: in `channel=check` run `44980898474` on `b0eb52c004` (#622), both Linux jobs (`137081069175`, `137081069176`) timed out 25 s after `hello`, and the log showed no engine output. The old script piped the engine's stderr and never read or printed it.

Changes under `.github/`:
- `.github/release/opencode-rune-verify-engine-sandbox.py`:
  - **Stderr:** the engine's stderr goes to a file, and stdout is drained after `listening`, so the engine can never block on a full pipe.
  - **Failure report:** on any failure the script prints the exit code, the stderr tail, the process tree (`Seccomp` and `NoNewPrivs` for each process) and the tail of Chromium's `debug.log`.
  - **Linux renderers:** they are found as children of a zygote process. Zygote-forked renderers keep `--type=zygote` in `/proc/<pid>/cmdline`, so the old line 138 never matched.
  - **macOS renderers:** they are found by the probe's own app-bundle path (`ps -axww`; both the `abspath` and `realpath` spellings, because `/var/folders/...` resolves to `/private/var/...`), not by parent pid. In the same run, `verify-engine-macos (x64, macos-26-intel)` (job `137082687621`) loaded the page and still failed at line 123 with `no macOS renderer subprocess found`. That is a second release blocker on `b0eb52c004`. The arm64 job passed. This change is not run on a Mac: no macOS device was used. If the renderer is truly missing, the failure report now prints every process running from the bundle, with `ppid` and `--type`.
  - **Required on both platforms:** `listening`, `ready`, and the test page loaded.
  - **Sandbox state:** printed for every renderer. It fails the check only with the new `--require-sandbox` flag.
- `.github/workflows/release.yml`:
  - **macOS:** `verify-engine-macos` passes `--require-sandbox`, so the macOS check is unchanged.
  - **Linux:** `build-engine-linux` runs the probe without the flag (report-only), and the step summary says so.
  - **Upload:** the Linux engine archive uploads even when the probe fails (`if: always() && steps.package-linux.outcome == 'success'`).

When the Linux sandbox returns in its own engine PR, a one-line follow-up patch adds `--require-sandbox` to the Linux call. That PR must also handle children of Chromium's unsandboxed zygote (utility processes, `Seccomp: 0` by design). The renderer filter counts children of both zygotes, so it would report those utility processes as "not sandboxed".

Tested on `vps` (Debian 13 x86_64, headless, Python 3.13) with the published revision-8 `linux-x64` engine (sha256 `4a62c658...` checked). Logs are kept in `C:\tmp\opencode\engine-targets\linux-probe\vps-0007-*.txt` and `proof\engine-targets\linux-probe\`:
- **Report-only:** exit 0. `listening` 0.03 s, `ready` 0.14 s, page loaded 0.24 s; 4 renderers printed `sandboxed=no Seccomp=0 NoNewPrivs=1`.
- **`--require-sandbox`:** exit 1 (revision 8 is unsandboxed), with the full failure report.
- **Fake engine that takes `hello` and never answers (the CI symptom):** exit 1, `no 'ready' within 25s after 'hello'`, with the exit code, process tree and stderr printed.
- **`python3 -m py_compile`:** OK.
- **Which revision of the script was tested:** the `vps` runs used its first revision (sha256 `cddade17...`). The final revision changes only the macOS functions and passes `bin_path` through; `py_compile` OK.
- **Patch applies:** `git apply --check` clean on `origin/dev` `b0eb52c004`.
- **Not run:** `actionlint` (not installed on blade).

```sh
git fetch origin && git checkout -b ci/engine-probe-report origin/dev
git am 0007-ci-release-Linux-engine-probe-proves-start-page-load.patch
git diff --stat origin/dev..HEAD && git rev-parse HEAD:.github
git push -u origin ci/engine-probe-report
```

- **Patch SHA-256:** `31da28942d9d4a8c0662e6c6e7446f62776e96c12135738dc97bca1dc9652429` (`18334` B)
- **Expected diffstat:** `2 files changed, 173 insertions(+), 54 deletions(-)`
- **Expected `HEAD:.github` (on `b0eb52c004`):** `3a39a1ed5f4b85f92175b0e3eb63f637b777a42e`
- **Numbering:** the orchestrator's note calls this "0005". `0005` and `0006` were already taken on this branch, so it is `0007`. The follow-up that adds `--require-sandbox` to the Linux call will be the next free number.
