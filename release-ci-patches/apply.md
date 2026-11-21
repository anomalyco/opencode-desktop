Tested on a fresh clone of `anomalyco/rune-desktop` at `origin/dev` `1dc6a78678309a8d45d4b87ae0a095852a43fe7d`. Merge **after #322** is on `dev`.
```sh
git fetch origin && git checkout -b ci/browser-engine-release origin/dev
git am 0001-ci-release-build-sign-publish-and-gate-browser-engin.patch 0002-ci-release-retire-manual-engine-linux-preview-jobs.patch
# Omit 0002 if you want to keep the manual engine-linux preview jobs.
git diff --stat origin/dev..HEAD && git rev-parse HEAD:.github
git push -u origin ci/browser-engine-release
gh pr create -R anomalyco/rune-desktop --base dev --head ci/browser-engine-release --title 'ci(release): build, sign, publish and gate browser engines' --body-file pr-body.md
```
Expected (`0001`+`0002`): `6 files changed, 1235 insertions(+), 69 deletions(-)`, `HEAD:.github` = `e96791c0192dd6fbd768d269ccfd295bd8644df7` (`0001` only: `6 files changed, 1234 insertions(+), 25 deletions(-)`, `06349dfadeec3877c2304cb0ccd91f42a1612340`).
First run after merge (dry run, builds/signs/verifies all 4 engines + desktop, publishes nothing):
`gh workflow run release.yml -R anomalyco/rune-desktop --ref dev -f channel=check`
