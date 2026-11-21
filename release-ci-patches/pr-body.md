**#322 is on `dev` at `f1f8a2194130d87e7a4b2d75de89b488d58ebafe`** (`crates/browser-pane/src/source.rs` release defaults: Windows and Linux x86_64 read `https://anoma.ly/release/opencode-rune/browser-engine/stable.json`; Windows trusts `publisher:Anomaly Innovations, Inc https://anoma.ly/`, macOS arm64 `publisher:5NZ4Q7NXJ4`, Linux x86_64 the `sha256:` pin `build-linux` sets). The gate's `source.rs` check passes on that `dev`; it fails on the live index until this workflow publishes `156.0.26-2`.

Merging to `dev` takes effect on the next scheduled run (`cron: "17 */6 * * *"` UTC) without manual dispatch: it builds, signs, verifies, and publishes the four `156.0.26-2` browser engines (`windows-x64`, `windows-arm64`, `macos-arm64`, `linux-x64`) to `opencode-rune/browser-engine/stable.json`, and from then on blocks `publish-desktop` whenever the publish gate fails.

### Commits

1. `77dbbf58` (`ci(release): build, sign, publish and gate browser engines`) — adds the engine build/sign/verify/publish jobs, `channel=engine`, `channel=check` engine dry-run, Linux `x64` `sha256:` pin, and `verify-engine-published` gate.
2. `ac4b9755` (`ci(release): retire manual engine-linux preview jobs`) — drops the unused `engine-linux` choice and `build-engine-linux` / `publish-engine-linux` jobs (`-46/+3` in `release.yml`). Optional commit on top; omit it to keep those preview jobs untouched.

### What it adds

- **Plan (`.github/release/opencode-rune-plan.sh`):** parses `apps/browser-engine/Cargo.lock` + `crates/browser-pane/src/installer.rs` (data-only sparse checkout), fetches live `stable.json` (fails on network/HTTP/JSON error), sets `build-engine=true` only when `stable.json` lacks this commit's `<cef>-<ENGINE_REVISION>` (`156.0.26-2`) or any of the four targets (or on `channel=check`/`channel=engine`), refuses `channel=engine` when the version is already complete, and adds `apps/browser-engine/` to the scheduled `app=` path filter.
- **Engine pipeline & atomic publish:** builds `windows-x64`, `windows-arm64`, `macos-arm64`, `linux-x64` (Windows resolves Visual Studio's bundled CMake/Ninja via `vswhere.exe` only with no `choco` install; macOS ensures `cmake`/`ninja` via Homebrew following `build-macos (x64)` `:174-176`'s `brew install nasm` precedent; Linux installs `build-essential cmake ninja-build` + `libcef.so` runtime libs); signs Windows (Azure Artifact Signing) and macOS (Developer ID + notarization + stapling); verifies signatures; publishes all four targets in one `release-client.sh publish --app opencode-rune --artifact browser-engine` request (`channel: stable`, `allow_prerelease: true`; authorized by `anomalyco/release` `registry/opencode-rune.json`, no registry change needed) under shared job concurrency `opencode-rune-browser-engine-publish` after re-checking `stable.json` is not already complete.
- **Linux `x64` pin & publish gate:** `build-linux` (`x64` only) waits on `build-engine-linux-x64` when `build-engine=true` and sets `OPENCODE_BROWSER_ENGINE_TRUST=sha256:<hex>` from `build-engine-linux-x64`'s `SHA256SUMS` (or live `stable.json` when `build-engine=false`). `verify-engine-published` runs `.github/release/opencode-rune-check-engine.sh` against hardcoded `https://anoma.ly/release/opencode-rune/browser-engine/stable.json` (exact version, four targets reachable with matching `Content-Length`, mandatory 64-hex `linux-x64` sha256 match, and `source.rs` release defaults for Windows, macOS arm64, and Linux x86_64) before `publish-desktop`.

### Security boundary (job by job)

Every new job pins `ref: ${{ github.sha }}`; zero new external actions are added.

- **`plan`** (no env/secrets/`id-token`): sparse-checks out `/.github/release/`, `apps/browser-engine/Cargo.lock`, `crates/browser-pane/src/installer.rs` as data only; regex-validates `CEF_BUILD`, `ENGINE_REVISION`, and `linux-x64-sha256` before writing `$GITHUB_OUTPUT`.
- **`credentials`** (`environment: release`, non-secret vars only): widened to `build-desktop == 'true' || build-engine == 'true'`.
- **`build-engine-windows`**, **`build-engine-macos`**, **`build-engine-linux-x64`** (no env/secrets/`id-token`): run repository packaging scripts and upload `SHA256SUMS`-digested artifacts via `./.github/release/upload`. `build-engine-macos` asserts `cmp -s .github/release/opencode-rune-engine.entitlements apps/browser-engine/entitlements.plist` before building. `build-engine-linux-x64` reads its `sha256` output directly from `upload`'s `SHA256SUMS`.
- **`sign-engine-windows`** (`environment: release`, `id-token: write`, sparse `/.github/release/` only): downloads verified artifacts via `./.github/release/download`; `.github/release/opencode-rune-package-engine-windows.ps1 -Mode Unpack` refuses tar traversal/links/reparse points and enforces the exact 5-file PE allowlist before signing; signs via `azure/login` + `azure/artifact-signing-action` (`cache-dependencies: false`); verifies with `.github/release/opencode-rune-verify-windows.ps1 -Publisher "Anomaly Innovations, Inc https://anoma.ly/"`; archives deterministically (`-Mode Archive`, ustar + zstd-19).
- **`sign-engine-macos`** (`environment: release`, `id-token: write`, sparse `/.github/release/` only): downloads verified artifact; step-scoped AWS OIDC (`output-env-credentials: false`) -> `with-secrets.sh` -> `macos-credentials.sh` with `if: always()` keychain removal; `.github/release/opencode-rune-sign-engine-macos.sh` enforces the exact 8-file sorted Mach-O allowlist, 5 helper `Info.plist` identifier/version checks, symlink confinement, and fail-hard notary credential check before inside-out signing with `.github/release/opencode-rune-engine.entitlements`, notarizing, stapling, and zipping.
- **`verify-engine-windows`**, **`verify-engine-macos`**, **`verify-engine-published`** (no env/secrets/`id-token`): sparse-check out `/.github/release/` (plus `features/updater/src/windows.rs` or `crates/browser-pane/src/source.rs` as data); verify Authenticode publisher, `stapler`/`spctl`/`codesign` team `5NZ4Q7NXJ4` + `notarized`, and live `stable.json` + `source.rs` defaults + Linux `x64` 64-hex pin.
- **`publish-engine`** (`environment: release`, `id-token: write`, sparse `/.github/release/` only, `concurrency: opencode-rune-browser-engine-publish`): downloads verified artifacts, validates `^[0-9a-f]{64}$` and confirms `SHA256SUMS` matches `build-engine-linux-x64.outputs.sha256`, re-checks `stable.json` is not complete, runs one `release-client.sh publish` call with `--verify-downloads`.

### Target gaps & Linux behavior

- **`macos-x64` (not done):** Intel Mac users continue seeing `"This build can't download the browser yet: it isn't published for this platform. An update will bring it."` (`source.rs:80`). Follow-up needs a target/arch parameter in `apps/browser-engine/scripts/package-release.sh`, resolving `cef_dir` without executing an x64 binary on arm64 (or building on `macos-26-intel`/Rosetta), x64 CEF, `source.rs` mapping, and matrixing the macOS engine jobs.
- **`linux-arm64`:** no browser by design of `apps/desktop/Cargo.toml:141-142` (`EngineStatus::Unsupported`).
- **Linux `x64` after a newer engine publishes:** an already-installed engine keeps running (background update check logs untrusted silently); a fresh install of an older Linux build shows `"This build doesn't trust the download, so nothing was installed."` (`source.rs:153-157`) on a same-revision CEF update (or `NotPublished` on a revision bump) until the desktop build is updated. Follow-ups: detached Ed25519 signature verification in `trust.rs`; check whether `com.apple.security.cs.disable-library-validation` can be dropped once a signed macOS engine exists.

### Proved vs not proved before merge (every new job)

- **Proved locally / on farm (scripts and checks, not CI runners):**
  - `plan`: `.github/release/opencode-rune-plan.sh` exercised against live `stable.json` (`dev`, `check`, `engine`, `schedule`).
  - `build-engine-macos`: entitlements `cmp -s` matched (`sha256: 3726221A...F466A4`); `package-release.sh --assemble-only` ran on `air` at `3c7ce6fa` (`12 s`, `130,377,795` B).
  - `sign-engine-macos` (pre-sign validation): `.github/release/opencode-rune-sign-engine-macos.sh` exercised on `air` against real assembled bundle + 5 negative mutations (extra Mach-O, renamed helper, abs/rel escaping symlinks, helper bad ID, missing notary creds).
  - `sign-engine-windows` (unpack/archive validation): `.github/release/opencode-rune-package-engine-windows.ps1` exercised locally against real unsigned `156.0.26.1` stage (`Unpack` + 4 negatives + byte-identical `Archive` `f722a74e...5535`); `opencode-rune-verify-windows.ps1` refused unsigned stage.
  - `verify-engine-published`: `.github/release/opencode-rune-check-engine.sh` exercised against live `stable.json` (fails on `156.0.14-1`), local loopback fixture + `ce06a33f` `source.rs` (passes), `d6cf8659` `source.rs` (fails), empty/missing/wrong `--linux-sha256` (fails), wrong `Content-Length`, missing target, bad JSON, `http://` & empty-Linux tampered `source.rs`, and non-loopback `--index-url` (all refused).
  - Workflow & script lint: `actionlint 1.7.12` exit 0, `shellcheck 0.11.0` exit 0, PowerShell AST 0 errors, `scripts/guardrails.ps1` 5/5.
- **Not proved before merge (CI runners, signing, publishing):**
  - Whether the Windows runner image's Visual Studio includes CMake and Ninja: if not, `build-engine-windows` fails at that step and nothing is published.
  - `build-engine-windows (x64)` CI job on `blacksmith-8vcpu-windows-2025`.
  - `build-engine-windows (arm64)` CI cross-build (`aarch64-pc-windows-msvc` + CMake/Ninja on `blacksmith-8vcpu-windows-2025`).
  - `build-engine-macos` CI job on `blacksmith-6vcpu-macos-26` (ran on `air`, not the Blacksmith macOS image).
  - `build-engine-linux-x64` CI job (`package-linux.sh` + `build-essential`/`cmake`/`ninja-build`/`libcef.so` runtime libs on `blacksmith-4vcpu-ubuntu-2404`).
  - `sign-engine-windows` (Azure Artifact Signing in `release` environment).
  - `sign-engine-macos` (AWS OIDC secret fetch, Developer ID `codesign`, Apple `notarytool`, `stapler` in `release` environment).
  - `verify-engine-windows` (CI unpack + Authenticode check on signed x64/arm64 `.tar.zst`).
  - `verify-engine-macos` (CI `stapler validate`, `spctl`, `codesign --verify` on signed arm64 `.zip`).
  - `publish-engine` (OIDC publish to `anoma.ly/release/opencode-rune/browser-engine/stable`).
  - `verify-engine-published` passing live against published `156.0.26-2` on `dev`.
  - `build-linux (x64)` compiling `opencode-desktop` with `OPENCODE_BROWSER_ENGINE_TRUST=sha256:<hex>` on `blacksmith-4vcpu-ubuntu-2404`.

### First run after merge

1. Dry run: `gh workflow run release.yml -R anomalyco/rune-desktop --ref dev -f channel=check`. Covers build, sign, and verify of all four engines and all six desktop targets (plus throwaway tag `v0.0.0-test-<n>`) and publishes nothing. Look for `build-engine-*`, `sign-engine-*`, `verify-engine-*`, and `build-linux (x64)` green. Failure modes: missing shared lib for `libcef.so` in `build-engine-linux-x64`, MSVC arm64/Ninja mismatch in `build-engine-windows (arm64)`, or codesign/notary error in `sign-engine-macos`.
2. Publish: dispatch `-f channel=engine` (publishes `156.0.26-2` with all four targets to `opencode-rune/browser-engine/stable.json`), then dispatch `-f channel=dev` or let the `17 */6 * * *` schedule run.

### Back-out

`git revert` the merge commit on `dev`. If reverted before publish, nothing changed externally. If reverted after `156.0.26-2` publishes, `stable.json` keeps `156.0.26-2` (or roll back via `anomalyco/release` `promote` with `sequence: 1`, `version: "156.0.14-1"`); apps with `156.0.26.2` installed keep running it, and older macOS builds pinned to `sha256:92291cae...` remain broken until updated to `publisher:5NZ4Q7NXJ4`.
