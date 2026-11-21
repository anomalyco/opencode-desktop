merge this and rune-desktop #399 together: the engine build compares the two files (`cmp -s .github/release/opencode-rune-engine.entitlements apps/browser-engine/entitlements.plist` in `.github/workflows/release.yml:721`).

Adds `com.apple.security.device.audio-input` and `com.apple.security.device.camera` to `.github/release/opencode-rune-engine.entitlements`, matching official `anomalyco/opencode` (`7f513b15b70e`: `.github/release/entitlements/desktop.plist:5-10`, `packages/desktop/electron-builder.config.ts:185-187`, `packages/app/src/runtime/i18n/desktop-native.ts:235-236`) and completing WS-1 #393 (`d97726ef`, `ENGINE_REVISION = 4`).

Tested via `git am` on a fresh clone of `origin/dev` `1758ae865a95b9d2ded13a2274e2f79f83d8ee23` (`actionlint` exit 0, LF, byte-identical to PR #399 `apps/browser-engine/entitlements.plist` SHA-256 `599fc23e7c84738aaee58080755b64a9ee79611fa501959c2cbeb2421de18399`).

```sh
git fetch origin && git checkout -b ci/engine-media-entitlements origin/dev
git am 0003-ci-release-allow-camera-and-microphone-in-macOS-brow.patch
git diff --stat origin/dev..HEAD && git rev-parse HEAD:.github
git push -u origin ci/engine-media-entitlements
```

- **Patch SHA-256:** `220a9b04015122905c824ca53e1ab3497c4066493673fb92b9ac124932a078ed` (`889` B)
- **Expected diffstat:** `1 file changed, 4 insertions(+)` (`.github/release/opencode-rune-engine.entitlements` only)
- **Expected `HEAD:.github`:** `21a84c600ae982ed2a6cfab2a0cb99713e23f81b`
- **Matching repo PR (hold until merged together with this patch):** `anomalyco/rune-desktop#399` (`feat/browser-engine-media-entitlements`, head `2a277be5a0a6e941190c1425d15c5491fe2c2399`)
