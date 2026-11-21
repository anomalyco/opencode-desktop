**Merge order: requires PR #357 on `dev`** (merged at `6d197b9e0e6440ce21b1a86aadab1c61acace47e`, which enables `macos-x64` and `linux-arm64` in `source.rs`, `apps/desktop/Cargo.toml`, `package-release.sh`, and `package-linux.sh`, and bumps `ENGINE_REVISION` `2 -> 3` so the six-target engine publishes as `156.0.26-3` without overwriting live `156.0.26-2`).

**First run after merge:** dispatch `channel=check` (`gh workflow run release.yml -R anomalyco/rune-desktop --ref dev -f channel=check`) before `channel=engine` or `channel=dev`. The six-target gate blocks desktop publishing until all six engines are published; if `macos-x64` or `linux-arm64` fails in the dry run, a repo-script PR or follow-up patch will land within the hour.

### What changes

- **`build-engine-macos` (`.github/workflows/release.yml`):** matrix across `arm64` (`blacksmith-6vcpu-macos-26`) and `x64` (`macos-26-intel`) with `fail-fast: false`, passing `--arch "${{ matrix.arch }}"` to `package-release.sh --assemble-only` and reporting `digest-arm64` / `digest-x64`.
- **`sign-engine-macos` & `verify-engine-macos` (`.github/workflows/release.yml`, `.github/release/opencode-rune-sign-engine-macos.sh`):** matrix per architecture (`sign-engine-macos` runs on `blacksmith-6vcpu-macos-26` like `sign-macos`; `verify-engine-macos` runs on `matrix.host` like `verify-macos`). `opencode-rune-sign-engine-macos.sh` accepts `arm64` and `x64` with the same exact 8-Mach-O / 5-helper allowlist and asserts `lipo -archs` matches `arm64` or `x86_64` on every allowlisted Mach-O (and `verify-engine-macos` checks the main binary arch alongside `stapler`, `spctl`, and `codesign`).
- **`build-engine-linux` & `build-linux` (`.github/workflows/release.yml`):** replaces `build-engine-linux-x64` with a `fail-fast: false` matrix across `x64` (`blacksmith-4vcpu-ubuntu-2404`) and `arm64` (`blacksmith-4vcpu-ubuntu-2404-arm`), adding `curl` to `apt-get install`, reporting `digest-{x64,arm64}` and `sha256-{x64,arm64}` from `SHA256SUMS`. Both `build-linux (x64)` and `build-linux (arm64)` wait on `build-engine-linux` and compile with `OPENCODE_BROWSER_ENGINE_TRUST=sha256:<arch hash>`.
- **`publish-engine`, `verify-engine-published`, `plan`, `check-engine` (`.github/workflows/release.yml`, `.github/release/opencode-rune-{plan,check-engine}.sh`):** require and publish all six targets atomically (`windows-x64`, `windows-arm64`, `macos-arm64`, `macos-x64`, `linux-x64`, `linux-arm64`); validate both `linux-x64` and `linux-arm64` 64-hex hashes across `plan`, `build-linux`, `publish-engine`, and `verify-engine-published`; `check_source_rs` verifies release defaults and `TARGET` entries for all six platforms.

### Security boundary (unchanged)

- Build and verify jobs run with no environment, secrets, or `id-token`.
- Sign and publish jobs stay in `environment: release`, sparse-check out `/.github/release/` at `github.sha` only, run only `.github/release/` scripts and existing SHA-pinned actions, and verify every artifact against its producing job's `SHA256SUMS` digest.

### Proved vs first proved by CI

- **Proved locally / on farm:**
  - `git am` on fresh clone of `dev` `6d197b9e0e6440ce21b1a86aadab1c61acace47e`: clean (`HEAD:.github = 88840f271f4495099cc8667257a356dd8a5998bd`).
  - `actionlint` (with embedded `shellcheck`): exit 0; `shellcheck -x` on all 3 modified `.github/release/*.sh`: exit 0; LF line endings verified.
  - 17/17 local fixture tests on `opencode-rune-check-engine.sh` and `opencode-rune-plan.sh` (live `156.0.26-2` incomplete under 6-target gate; 6-target loopback fixture passes with PR #357 `source.rs`, refuses old `39bcbb55` `source.rs`, missing/wrong `linux-x64` or `linux-arm64` hashes, missing `macos-x64` or `linux-arm64` targets, tampered `TARGET`; `plan` produces `156.0.26-3` / `build-engine=true` for `check`/`dev`/`engine`).
  - Inspected real Spotify CEF `156.0.26+g8ea0c73+chromium-156.0.8078.88` minimal tarballs for `macosx64` (exact 8 Mach-Os / 5 helpers after strip) and `linuxarm64` (exact runtime layout).
  - `aarch64-unknown-linux-gnu` cross type-check on farm: `opencode-desktop` + `browser-pane` CLEAN, `apps/browser-engine --bins` CLEAN.
- **First proved by CI (`channel=check`):**
  - `build-engine-macos (x64)` on `macos-26-intel`, `sign-engine-macos (x64)`, `verify-engine-macos (x64)`.
  - `build-engine-linux (arm64)` on `blacksmith-4vcpu-ubuntu-2404-arm` and `build-linux (arm64)` with `OPENCODE_BROWSER_ENGINE_TRUST=sha256:...`.
  - Six-target `publish-engine` and `verify-engine-published` (`channel=engine` / `channel=dev`).
