# Apply 0009: retry `xcrun stapler validate` in the macOS verify steps (from RELEASE-CI, 22 Nov)

- **Patch:** `0009-ci-release-retry-xcrun-stapler-validate-up-to-three-.patch` (SHA-256 `3fcad1683272118a3f2be2b5e202a0d9435af044f81efa51d4f1cec276756121`, 1 file, `+21/-3`), made against `dev` `08199534f8`.
- **Why:** `verify-engine-macos (arm64)` in run `44998875366` (job `137137476836`) failed at `xcrun stapler validate` with `CloudKit query … failed due to "(null)"`, error 65. That is Apple's ticket lookup failing, not our bytes. One such hiccup stops a publish.
- **Change, `.github/workflows/release.yml` only:**
  - A small `stapler_validate` function in the two verify steps that call `xcrun stapler validate`: `verify-macos` ("Verify signed app, updater zip, and DMG", 2 calls) and `verify-engine-macos` ("Verify signed macOS … browser engine", 1 call).
  - It tries up to 3 times with 30 s between tries, prints a `::warning::` for each failed try, and fails the step only if all 3 fail. Worst added time: 60 s.
  - Every other check in those steps is unchanged (`spctl --assess`, `codesign --verify --deep --strict -R … notarized`, and so on).
- **Not changed (your call):** `.github/release/opencode-rune-sign-macos.sh:54-55,66-67` and `opencode-rune-sign-engine-macos.sh:171-172` also run `xcrun stapler staple` and `validate` in the sign jobs. The same lookup can fail there. I left them alone because they run in the `release` environment.
- **Checked:**
  - `git apply --check` is clean on `origin/dev` `08199534f8`.
  - `Bun.YAML.parse` reads the patched file and finds the function in both steps.
  - `bash -n` passes on both `run:` blocks.
  - The exact function, run under `bash -e` with a fake `xcrun` and no-op `sleep`, gives:
    - fails twice then succeeds: passes after 3 calls;
    - fails three times: the step fails;
    - succeeds first time: 1 call.
- **Apply:** `git am engine-targets-patches/0009-ci-release-retry-xcrun-stapler-validate-up-to-three-.patch`
