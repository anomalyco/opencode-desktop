Independent of #622 / patch 0004 (can land anytime): fixes the gap where PRs touching only `apps/browser-engine/**` (e.g. #595) skipped `opencode-rune-pr-build.yml` entirely, and ensures PRs touching `apps/browser-engine/**` also build and attach an unsigned Windows x64 browser engine archive so `tools\smoke.ps1 <PR>` can test app + that PR's engine before merge.

Changes under `.github/`:
- `.github/workflows/opencode-rune-pr-build.yml`:
  - Adds `apps/browser-engine/**` to `on.pull_request.paths`.
  - Grants `plan` read-only `pull-requests: read` and exposes `build-engine` output.
  - Adds `build-engine-windows-x64` job (`blacksmith-8vcpu-windows-2025`, zero secrets/environment, runs ONLY when `needs.plan.outputs.build-engine == 'true'`), building with `apps/browser-engine/scripts/package-windows.ps1 -Arch x64` and uploading `build-opencode-rune-pr-engine-windows-x64`.
  - Updates `publish-pr-build` to attach `OpenCode-Rune-PR-<N>-engine-windows-x64.tar.zst` to the PR pre-release and link it in the sticky comment when built (skipped cleanly on non-engine PRs).
- `.github/release/opencode-rune-pr-plan.sh`: detects whether any changed file in the PR starts with `apps/browser-engine/` (paginated `gh api`, SIGPIPE-safe under `set -euo pipefail`) and emits `build-engine=true|false`.

Runner-minute cost:
- **Non-engine PRs** (no `apps/browser-engine/**` changes): **0 extra runner minutes** (`build-engine-windows-x64` is skipped; `plan` adds <1 s for one GitHub API call).
- **Mixed PRs** (already triggering `opencode-rune-pr-build.yml` and also touching `apps/browser-engine/**`): **+1.8 to 2.0 runner minutes** on `blacksmith-8vcpu-windows-2025` (measured from `release.yml` `build-engine-windows (x64)` runs `44862185681` = `1m51s` and `44865312737` = `1m43s`; cold uncached CEF build ~5-8 min). Runs in parallel with `build-windows` (~23 min) and `build-macos` (~15-20 min), adding **0 min** to PR critical-path wall time.
- **Engine-only PRs** (previously skipped `opencode-rune-pr-build.yml` entirely): adds the standard PR build (~15-20 min `build-macos` + ~23 min `build-windows` x2 + ~2 min `build-engine-windows-x64`) so `tools\smoke.ps1 <PR>` has both desktop and engine artifacts.

Tested via `git am` on a fresh detached checkout of `origin/dev` `98544f6b5ddb84384400e9d1f3c7095544e4452a`:
- `actionlint` + `shellcheck` on `opencode-rune-pr-build.yml`: exit 0
- `shellcheck` on `opencode-rune-pr-plan.sh`: exit 0

```sh
git fetch origin && git checkout -b ci/pr-build-browser-engine origin/dev
git am 0005-ci-pr-build-build-and-attach-Windows-x64-browser-eng.patch
git diff --stat origin/dev..HEAD && git rev-parse HEAD:.github
git push -u origin ci/pr-build-browser-engine
```

- **Patch SHA-256:** `f571c627fde0a75c4fd8a8004c368e7b98662541b12939b8d8ea39ef5f2728f7` (`8992` B)
- **Expected diffstat:** `2 files changed, 87 insertions(+), 4 deletions(-)`
- **Expected `HEAD:.github` (on `98544f6b5ddb84384400e9d1f3c7095544e4452a`):** `8277b1b7ab2949fecadcc4e35acd5f790d591669`