# Releasing OpenCode (Rune)

`.github/workflows/release.yml` builds, signs and publishes the desktop app to `https://anoma.ly/release` (app `opencode`, artifact `desktop-rune`: `opencode/desktop-rune/<channel>`). It follows the [infrastructure guide](https://anoma.ly/team/abattoir/guides/infrastructure), with Hex as the closest example.

## Channels

- **dev**: checks `dev` every six hours (`17 */6 * * *` UTC). A scheduled run skips the builds when no app files changed; dispatching `dev` builds immediately. Each release publishes `0.0.0-dev-<n>` with all 14 targets. A failed build, signing step or package check leaves the previous complete release available. The unsigned Windows setups also go to the rolling `opencode-rune-windows-dev` pre-release.
- **check**: dispatch from `dev`. It builds, signs and verifies, and publishes nothing. Its last job pushes and deletes a throwaway `v0.0.0-test-<n>` tag.
- **stable**: dispatch from `dev` with a bump (`patch`, `minor` or `major`). The version is the latest `vX.Y.Z` tag bumped (`0.0.0` when there is none). After publishing the same target set as dev, the last job tags `v<version>` on the released commit and mirrors the packages to the GitHub release.

```sh
gh workflow run release.yml -R anomalyco/rune-desktop --ref dev -f channel=stable -f bump=patch
```

Tags start nothing, and the workflow has no ref input: a run always releases `github.sha` on `dev`.

## Latest downloads

Each link redirects to the channel's current file. Append `/info` to a link for its version, byte size and SHA-256. The links resolve once that channel has published the target.

| Platform | Dev | Stable |
| --- | --- | --- |
| macOS Apple Silicon | [DMG](https://anoma.ly/release/opencode/desktop-rune/dev/macos-arm64-dmg) · [update ZIP](https://anoma.ly/release/opencode/desktop-rune/dev/macos-arm64) | [DMG](https://anoma.ly/release/opencode/desktop-rune/stable/macos-arm64-dmg) · [update ZIP](https://anoma.ly/release/opencode/desktop-rune/stable/macos-arm64) |
| macOS Intel | [DMG](https://anoma.ly/release/opencode/desktop-rune/dev/macos-x64-dmg) · [update ZIP](https://anoma.ly/release/opencode/desktop-rune/dev/macos-x64) | [DMG](https://anoma.ly/release/opencode/desktop-rune/stable/macos-x64-dmg) · [update ZIP](https://anoma.ly/release/opencode/desktop-rune/stable/macos-x64) |
| Windows x64 | [setup](https://anoma.ly/release/opencode/desktop-rune/dev/windows-x64-setup) · [update ZIP](https://anoma.ly/release/opencode/desktop-rune/dev/windows-x64) | [setup](https://anoma.ly/release/opencode/desktop-rune/stable/windows-x64-setup) · [update ZIP](https://anoma.ly/release/opencode/desktop-rune/stable/windows-x64) |
| Windows arm64 | [setup](https://anoma.ly/release/opencode/desktop-rune/dev/windows-arm64-setup) · [update ZIP](https://anoma.ly/release/opencode/desktop-rune/dev/windows-arm64) | [setup](https://anoma.ly/release/opencode/desktop-rune/stable/windows-arm64-setup) · [update ZIP](https://anoma.ly/release/opencode/desktop-rune/stable/windows-arm64) |
| Linux x64 | [AppImage](https://anoma.ly/release/opencode/desktop-rune/dev/linux-x64-appimage) · [DEB](https://anoma.ly/release/opencode/desktop-rune/dev/linux-x64-deb) · [RPM](https://anoma.ly/release/opencode/desktop-rune/dev/linux-x64-rpm) | [AppImage](https://anoma.ly/release/opencode/desktop-rune/stable/linux-x64-appimage) · [DEB](https://anoma.ly/release/opencode/desktop-rune/stable/linux-x64-deb) · [RPM](https://anoma.ly/release/opencode/desktop-rune/stable/linux-x64-rpm) |
| Linux arm64 | [AppImage](https://anoma.ly/release/opencode/desktop-rune/dev/linux-arm64-appimage) · [DEB](https://anoma.ly/release/opencode/desktop-rune/dev/linux-arm64-deb) · [RPM](https://anoma.ly/release/opencode/desktop-rune/dev/linux-arm64-rpm) | [AppImage](https://anoma.ly/release/opencode/desktop-rune/stable/linux-arm64-appimage) · [DEB](https://anoma.ly/release/opencode/desktop-rune/stable/linux-arm64-deb) · [RPM](https://anoma.ly/release/opencode/desktop-rune/stable/linux-arm64-rpm) |

The Mac and Windows updaters read the bare `<os>-<arch>` ZIP targets. Linux uses manual downloads; its packages are built on Ubuntu 24.04. Set the AppImage's executable bit before launching it (`chmod +x`). The release registry authorizes the artifact and channels; adding a target requires no registry change.

## Comparison with the official desktop

Source: [official release workflow](https://github.com/anomalyco/opencode/blob/38bd73789a43bf043599d6196af96583228035d7/.github/workflows/release.yml) and [desktop publisher](https://github.com/anomalyco/opencode/blob/38bd73789a43bf043599d6196af96583228035d7/.github/release/publish-desktop.sh).

| | Official | Rune |
| --- | --- | --- |
| Cadence | Six-hour dev schedule; beta on push; dispatch from dev for latest | Six-hour dev schedule with app-change gate; dispatch for dev, check or stable |
| macOS | arm64 + x64 DMG and ZIP; legacy `.app.tar.gz` on latest | arm64 + x64 DMG and ZIP |
| Windows | x64 + arm64 signed installer EXE | x64 + arm64 signed setup EXE and signed application ZIP |
| Linux | x64 + arm64 AppImage, DEB, RPM; per-architecture Electron update feeds | Same architectures and packages; manual updates |
| Signing | Apple Developer ID and notarization; Azure Authenticode; unsigned Linux packages | Same signing policy; every artifact checked against the producing job's SHA256SUMS digest |
| Publishing | `opencode.ai/files` (R2); `desktop` / distribution `opencode` at `opencode.ai/update`; `dev`, `beta`, `latest` | `anoma.ly/release/opencode/desktop-rune`; `dev`, `stable`; target names in the links above |

On a fully active day the schedule can build 24 OS/architecture combinations, up from seven (five Windows x64, one Windows arm64 and one Mac arm64). The change gate keeps idle runs to one Linux planning job, and concurrency lets the active release finish with only the newest pending run per channel. If runner spend exceeds budget, reduce the number of scheduled releases while retaining every target in each publish.

## Linux browser engine preview

Dispatch `release.yml` from `dev` with `channel=engine-linux` to build the x86_64 CEF sidecar and publish a private GitHub prerelease. Its `index.json`, `.tar.zst` and `SHA256SUMS` can be downloaded together with `gh release download <tag>`. A debug desktop build accepts that local index with `OPENCODE_BROWSER_ENGINE_SOURCE=file:$PWD/index.json OPENCODE_BROWSER_ENGINE_TRUST=local`.

The desktop starts without CEF. The embedded browser has a separate delivery path: `crates/browser-pane/src/source.rs` pins the public macOS arm64 archive; Windows defaults to a local preview index. Linux x64 has a private preview, but release builds have no Linux source or trust pin. The installer also needs target support for Linux arm64 and Intel Mac. To enable public delivery, publish the engine targets together (the index is replaced whole) and configure a trusted source and archive hashes in the desktop build. The Linux packages do not enable the embedded browser.

## Security boundary

- Build and verify jobs run repository code with no environment, secrets or `id-token`.
- Sign and publish jobs use the `release` environment, which can deploy only from `dev`. They sparse-check out only `.github/release/` at `github.sha` and treat artifacts as data, checked against the SHA256SUMS digest the job that built them reported.
- Secrets live in AWS Secrets Manager (`anomaly-secrets`, `604439173067`). GitHub OIDC assumes `github/rune-desktop-release`, pinned to `release.yml@refs/heads/dev` and environment `release` in `anomalyco/secrets` `config.ts`. The Apple certificate and notary key are `shared/APPLE_SIGNING_CERTIFICATE` and `shared/APPLE_NOTARY_KEY`. They're read in the one step that uses them (`.github/release/with-secrets.sh`) and never reach `$GITHUB_ENV`.
- The `release` environment holds only non-secret variables: `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_SUBSCRIPTION_ID`, `AZURE_TRUSTED_SIGNING_ENDPOINT`, `AZURE_TRUSTED_SIGNING_ACCOUNT_NAME`, `AZURE_TRUSTED_SIGNING_CERTIFICATE_PROFILE`, `APPLE_NOTARY_KEY_ID` and `APPLE_NOTARY_ISSUER_ID`. Repository and environment secrets stay empty.
- The release service accepts publishes only from `anomalyco/rune-desktop`, `.github/workflows/release.yml`, `refs/heads/dev` and environment `release` (`anomalyco/release` `registry/opencode.json`).
- Changes to `.github/` need Dax (`.github/CODEOWNERS` and the `release boundary (.github needs Dax)` ruleset).
- PR builds are never signed with release credentials. Their macOS app is ad-hoc signed.
