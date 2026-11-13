# Releasing OpenCode (Rune)

`.github/workflows/release.yml` builds, signs and publishes the desktop app to `https://anoma.ly/release` (app `opencode`, artifact `desktop-rune`: `opencode/desktop-rune/<channel>`). It follows the [infrastructure guide](https://anoma.ly/team/abattoir/guides/infrastructure), with Hex as the closest example.

## Channels

- **dev**: every push to `dev` that changes an app file builds every platform (Windows x64 and arm64, macOS), signs it, and publishes `0.0.0-dev-<n>` on the dev channel. A running release finishes, and of the pushes that land meanwhile only the newest waits. The unsigned Windows setups also go to the rolling `opencode-rune-windows-dev` pre-release.
- **check**: dispatch from `dev`. It builds, signs and verifies, and publishes nothing. Its last job pushes and deletes a throwaway `v0.0.0-test-<n>` tag.
- **stable**: dispatch from `dev` with a bump (`patch`, `minor` or `major`). The version is the latest `vX.Y.Z` tag bumped (`0.0.0` when there is none). Every platform has to be signed and verified. Once it's published, the last job tags `v<version>` on the released commit and creates the GitHub release with the signed files. A failed run doesn't use up a version.

```sh
gh workflow run release.yml -R anomalyco/rune-desktop --ref dev -f channel=stable -f bump=patch
```

Tags start nothing, and the workflow has no ref input: a run always releases `github.sha` on `dev`.

## Linux browser engine preview

Dispatch `release.yml` from `dev` with `channel=engine-linux` to build the x86_64 CEF sidecar and publish a private GitHub prerelease. Its `index.json`, `.tar.zst` and `SHA256SUMS` can be downloaded together with `gh release download <tag>`. A debug desktop build accepts that local index with `OPENCODE_BROWSER_ENGINE_SOURCE=file:$PWD/index.json OPENCODE_BROWSER_ENGINE_TRUST=local`.

The public `opencode-rune/browser-engine/stable` index currently contains the pinned macOS bundle. Publishing a release-service channel replaces every target. Public delivery needs a coordinated engine release and a Linux archive hash pinned in the desktop build.

## Security boundary

- Build and verify jobs run repository code with no environment, secrets or `id-token`.
- Sign and publish jobs use the `release` environment, which can deploy only from `dev`. They sparse-check out only `.github/release/` at `github.sha` and treat artifacts as data, checked against the SHA256SUMS digest the job that built them reported.
- Secrets live in AWS Secrets Manager (`anomaly-secrets`, `604439173067`). GitHub OIDC assumes `github/rune-desktop-release`, pinned to `release.yml@refs/heads/dev` and environment `release` in `anomalyco/secrets` `config.ts`. The Apple certificate and notary key are `shared/APPLE_SIGNING_CERTIFICATE` and `shared/APPLE_NOTARY_KEY`. They're read in the one step that uses them (`.github/release/with-secrets.sh`) and never reach `$GITHUB_ENV`.
- The `release` environment holds only non-secret variables: `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_SUBSCRIPTION_ID`, `AZURE_TRUSTED_SIGNING_ENDPOINT`, `AZURE_TRUSTED_SIGNING_ACCOUNT_NAME`, `AZURE_TRUSTED_SIGNING_CERTIFICATE_PROFILE`, `APPLE_NOTARY_KEY_ID` and `APPLE_NOTARY_ISSUER_ID`. Repository and environment secrets stay empty.
- The release service accepts publishes only from `anomalyco/rune-desktop`, `.github/workflows/release.yml`, `refs/heads/dev` and environment `release` (`anomalyco/release` `registry/opencode.json`).
- Changes to `.github/` need Dax (`.github/CODEOWNERS` and the `release boundary (.github needs Dax)` ruleset).
- PR builds are never signed with release credentials. Their macOS app is ad-hoc signed.
