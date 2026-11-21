**Order & gate warning — read first:**
1. **Apply ONLY after PR #357 is on `dev`** (already merged at `6d197b9e0e6440ce21b1a86aadab1c61acace47e`, bumping `ENGINE_REVISION` `2 -> 3` for six-target engine `156.0.26-3`). With six required targets, the publish gate blocks every desktop release until all six engines are published.
2. **Run `channel=check` first** (`gh workflow run release.yml -R anomalyco/rune-desktop --ref dev -f channel=check`) before `channel=engine` or `channel=dev`.
3. **If `macos-x64` or `linux-arm64` fails in that dry run, ENGINE-TARGETS fixes it within the hour** as a repo-script PR or a further workflow patch.

Tested via `git am` on a fresh clone of `anomalyco/rune-desktop` at `origin/dev` `6d197b9e0e6440ce21b1a86aadab1c61acace47e` (`actionlint` exit 0, `shellcheck -x` exit 0, 17/17 local gate/plan fixture tests pass).

```sh
git fetch origin && git checkout -b ci/engine-targets origin/dev
git am 0001-ci-release-build-sign-publish-and-gate-macos-x64-and.patch
git diff --stat origin/dev..HEAD && git rev-parse HEAD:.github
git push -u origin ci/engine-targets
gh pr create -R anomalyco/rune-desktop --base dev --head ci/engine-targets --title 'ci(release): build, sign, publish and gate macos-x64 and linux-arm64 browser engines' --body-file pr-body.md
```

- **Patch SHA-256:** `4a889b9999a31aee6206b0586b7ec51d88cf04424241623abc73c62fbfc0b948` (`44,742` B)
- **Expected diffstat:** `4 files changed, 264 insertions(+), 126 deletions(-)`
- **Expected `HEAD:.github`:** `88840f271f4495099cc8667257a356dd8a5998bd`
- **First run after merge (dry run, builds/signs/verifies all 6 engines + 6 desktop builds, publishes nothing):**
  `gh workflow run release.yml -R anomalyco/rune-desktop --ref dev -f channel=check`
