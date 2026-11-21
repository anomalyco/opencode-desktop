merge this and rune-desktop #622 together: #622 replaces `opencode-browser-engine-helper.exe` with `opencode-browser-engine.dll` (loaded by CEF's `bootstrap.exe` renamed to `opencode-browser-engine.exe` with `SizeOfStackReserve >= 8 MiB`) to enable Windows OS sandboxing. Merging either side alone breaks `sign-engine-windows` / `verify-engine-windows` on the 5-PE allowlist.

Changes under `.github/`:
- `.github/release/opencode-rune-package-engine-windows.ps1`: updates the 5-PE allowlist and archive order from `opencode-browser-engine-helper.exe` to `opencode-browser-engine.dll`, and enforces `SizeOfStackReserve >= 8 MiB` (`#582`) on `opencode-browser-engine.exe`.
- `.github/release/opencode-rune-verify-engine-windows-sandbox.ps1`: live signed Windows `x64` sandbox probe (verifies `CheckDllCodeSigning` + `renderer` token `Untrusted (0x0000)` with `RestrictedSids >= 1`). Live-tested on `blade` (exit 0).
- `.github/release/opencode-rune-verify-engine-sandbox.py`: live macOS (`sandbox_check(pid, NULL, 0) == 1`) and Linux (`Seccomp: 2` + `NoNewPrivs: 1`) sandbox probe.
- `.github/workflows/release.yml`: updates `verify-engine-windows` PE list + 8 MiB stack check + live `x64` sandbox probe; adds `libcef_sandbox.dylib` presence + live Seatbelt probe to `verify-engine-macos` (runs on both `macos-arm64` and `macos-26-intel`); adds conditional `unshare -Ur` live sandbox probe to `build-engine-linux` (`x64` + `arm64`).

Tested via `git am` on a fresh detached checkout of `origin/dev` `37cb309ffe9839bb4929ba66414d8122c3e6e165`:
- `actionlint` + `shellcheck`: exit 0
- PowerShell AST parser on both `.ps1` files: 0 errors
- `python -m py_compile` on `.py`: exit 0
- Live Windows sandbox probe script on `blade`: exit 0 (`renderer integrity_rid:restricted_sids=0:1`)

```sh
git fetch origin && git checkout -b ci/browser-engine-sandbox origin/dev
git am 0004-ci-release-update-Windows-engine-PE-allowlist-for-sa.patch
git diff --stat origin/dev..HEAD && git rev-parse HEAD:.github
git push -u origin ci/browser-engine-sandbox
```

- **Patch SHA-256:** `8249c32bc4f4e1bb9763bd61188c545f68cb8aebc24fbd1c96f96951a76da249` (`20484` B)
- **Expected diffstat:** `4 files changed, 370 insertions(+), 3 deletions(-)`
- **Expected `HEAD:.github` (on `37cb309ff`):** `59663bb041b17d206e7e579b9400b911bcec4bd0`
- **Matching repo PR (hold until READER-7 read + merged together with this patch):** `anomalyco/rune-desktop#622` (`feat/browser-engine-sandbox`, head `5026f81b788e52c59f8bb3e2e66c6e039db66f49`)
- **Note on Windows `arm64` runtime:** `verify-engine-windows` runs on `blacksmith-4vcpu-windows-2025` (x64), so `arm64` is statically checked (Authenticode + 5-PE allowlist + 8 MiB stack reserve); live `arm64` execution requires a native `windows-11-arm` runner if available.