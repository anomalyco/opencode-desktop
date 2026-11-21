Stacks cleanly on top of ENGINE-TARGETS' patch `0005` (`ci(pr-build): build and attach Windows x64 browser engine when apps/browser-engine changes`): moves the 12-check Windows x64 launch smoke off `blade`'s contended capture queue into GitHub Actions as its own PR check (`smoke-windows-x64`) on `blacksmith-8vcpu-windows-2025`, testing the exact desktop artifact built by `build-windows (x64)` against either the PR's own `0005` browser engine (for engine PRs) or the cached SHA-256-verified published browser engine (for non-engine PRs).

Changes under `.github/`:
- `.github/workflows/opencode-rune-pr-build.yml`:
  - Adds `smoke-windows-x64` (`needs: [plan, build-windows, build-engine-windows-x64]`, `runs-on: blacksmith-8vcpu-windows-2025`, `timeout-minutes: 15`, `permissions: { contents: read }`, zero secrets/environment/write tokens).
  - Checks out `/.github/release/` strictly from `github.event.pull_request.base.sha` so a PR cannot alter its own smoke check.
  - Unpacks `build-opencode-rune-pr-windows-x64` (`needs.build-windows.outputs.digest-x64`), pairs it with `build-opencode-rune-pr-engine-windows-x64` (when `build-engine == 'true'`, from `0005`) or the SHA-256-verified published engine from `https://anoma.ly/release/opencode-rune/browser-engine/stable.json` (cached via `actions/cache` keyed on `smoke-browser-engine-win-x64-<sha256>`).
  - Runs `.github/release/opencode-rune-smoke-windows.ts` directly (no hidden desktop needed on a CI runner), posts the 12-line summary table to `$GITHUB_STEP_SUMMARY`, and exits `0` iff all 12/12 checks pass.
- `.github/release/opencode-rune-smoke-windows.ts`:
  - Self-contained CI port of `harness/smoke.ts` with an in-process mock OpenCode HTTP/SSE server and a deterministic generated MV3 test popup extension (zero dependency on `operation-detox`, `sweep-fixture`, `opencode-dev`, or external CRX downloads).
  - Covers all 12 checks (`first-frame`, `plugins`, `launcher`, `browser`, `terminal`, `settings`, `session`, `prompt`, `element-comment`, `queued-prompt`, `extension-popup`, `quit` with own-PID/`BASE`-scoped `MDMP` crash-dump inspection).
  - *First-run check note:* On Windows runners without a discrete GPU, `wgpu` falls back to Microsoft WARP; confirm on the first CI run whether software rendering needs any extra adapter flag or slightly wider pass margins (`+2–3 s`).

Runner-minute cost:
- **Per PR push triggering `build-windows (x64)`:** **~2.5–3.5 runner minutes** on `blacksmith-8vcpu-windows-2025`:
  - Checkout + `setup-bun` + download/unpack desktop setup: ~35–45 s
  - Browser engine restore (warm `actions/cache` hit on published engine, or `0005` PR engine download/unpack): ~10–25 s (cold published fetch ~35 s)
  - 12-check `opencode-rune-smoke-windows.ts` run (warm SQLite reuse across passes): ~90–130 s (~1.5–2.2 min)
- Runs in parallel with `publish-pr-build` (or can be added to `publish-pr-build.needs` if you want smoke to gate pre-release publishing), adding ~3 min to total PR check wall time while removing 100% of PR smoke traffic from `blade`'s capture queue.

Tested via `git am` on a fresh detached checkout of `origin/dev` `d1bf2bcea24116bf67f430d5c59fcc44e5561d73` + `0005`:
- `git am 0006-...patch`: exit 0
- `Bun.YAML.parse` on `.github/workflows/opencode-rune-pr-build.yml`: exit 0
- `bun build .github/release/opencode-rune-smoke-windows.ts --target bun`: exit 0

```sh
git fetch origin && git checkout -b ci/pr-smoke-windows-x64 origin/dev
git am 0005-ci-pr-build-build-and-attach-Windows-x64-browser-eng.patch
git am 0006-ci-pr-build-run-12-check-Windows-x64-smoke-job-on-PR.patch
git diff --stat origin/dev..HEAD && git rev-parse HEAD:.github
git push -u origin ci/pr-smoke-windows-x64
```

- **Patch SHA-256:** `4266886642c901f495ccc6fa8c5dbe3a87a39cf773a988a382eaeae1f25dd341` (`42,436` B)
- **Expected diffstat (`0006` alone):** `2 files changed, 770 insertions(+)`
- **Expected `HEAD:.github` (on `d1bf2bcea24116bf67f430d5c59fcc44e5561d73` + `0005` + `0006`):** `2203c77ad2d3ca2d4c6351a9c9e7ed342e17ba7a`
