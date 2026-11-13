#!/usr/bin/env bash
# opencode-rune-plan.sh: decides what .github/workflows/release.yml builds and publishes, from the
# event, the ref and the repository's tags alone.
#
#   dev     a push to dev (or a dispatch from dev): 0.0.0-dev-<run + 400> on the release
#           service's dev channel, signed, every platform (SCOPE all). A dispatch can pick SCOPE
#           x64 (Windows x64 only); a channel's release is replaced whole, so a platform that run
#           didn't build has no target until a run builds it again (the updater then reports
#           NoTarget, updater/protocol.rs `decide`).
#   check   a dispatch from dev: the same build, signed and verified, published nowhere. Its last
#           job pushes and deletes a throwaway v0.0.0-test-<run> tag to prove it can tag.
#   stable  a dispatch from dev with BUMP (patch, minor or major): the latest vX.Y.Z tag (0.0.0
#           when there is none) bumped, on the stable channel, every platform. Tags start nothing:
#           the workflow tags v<version> and creates the GitHub release only after publishing.
#
# A run releases the commit it runs on: SHA (the checked-out commit) must be GITHUB_SHA, so every
# job checks out github.sha.
#
# Environment: EVENT, REF_NAME, CHANNEL, BUMP, SCOPE, SHA, GITHUB_SHA, GITHUB_RUN_NUMBER,
# GITHUB_REPOSITORY, GH_TOKEN.
. "$(dirname "$0")/lib.sh"

[ -n "$SHA" ] && [ "$SHA" = "${GITHUB_SHA:-}" ] || die "The checked-out commit $SHA is not this run's commit ${GITHUB_SHA:-}."
[ "$REF_NAME" = dev ] || die "Releases run from dev, not $REF_NAME."

tag=""
scope=${SCOPE:-all}
# Renaming the workflow to release.yml (2026-11-12) restarted its run numbers at 1, after
# opencode-rune-release.yml had reached 386. Continue past them, so no two builds share a version.
number=$((GITHUB_RUN_NUMBER + 400))

case "$CHANNEL" in
  stable)
    [ "$EVENT" = workflow_dispatch ] || die "Stable releases are dispatched from dev."
    latest=$(gh api "repos/$GITHUB_REPOSITORY/git/matching-refs/tags/v" --paginate --jq '.[].ref' |
      sed -n 's|^refs/tags/v\([0-9]*\.[0-9]*\.[0-9]*\)$|\1|p' | sort -V | tail -n 1)
    IFS=. read -r major minor patch <<< "${latest:-0.0.0}"
    case "${BUMP:-patch}" in
      major) version="$((major + 1)).0.0" ;;
      minor) version="$major.$((minor + 1)).0" ;;
      patch) version="$major.$minor.$((patch + 1))" ;;
      *) die "Unknown bump $BUMP." ;;
    esac
    tag="v$version"
    refs=$(gh api "repos/$GITHUB_REPOSITORY/git/matching-refs/tags/$tag" --jq '.[].ref')
    if grep -Fxq "refs/tags/$tag" <<< "$refs"; then die "$tag is already tagged."; fi
    release_channel=stable release_version=$version
    notes="https://github.com/$GITHUB_REPOSITORY/releases/tag/$tag"
    scope=all
    ;;
  dev)
    release_channel=dev release_version="0.0.0-dev-$number"
    notes="https://github.com/$GITHUB_REPOSITORY/commit/$SHA"
    ;;
  check)
    release_channel=dev release_version="0.0.0-dev-$number"
    tag="v0.0.0-test-$number"
    notes="https://github.com/$GITHUB_REPOSITORY/commit/$SHA"
    ;;
  *) die "Unknown channel $CHANNEL." ;;
esac

case "$scope" in x64 | all) ;; *) die "Unknown scope $scope." ;; esac

if [ "$scope" = all ]; then windows_arches='["x64","arm64"]' macos=true; else windows_arches='["x64"]' macos=false; fi

{
  echo "channel=$CHANNEL"
  echo "service=https://anoma.ly/release"
  echo "tag=$tag"
  echo "notes-url=$notes"
  echo "release-channel=$release_channel"
  echo "release-version=$release_version"
  echo "windows-arches=$windows_arches"
  echo "macos=$macos"
} >> "$GITHUB_OUTPUT"
echo "Planned OpenCode (Rune) ($CHANNEL, $scope): $release_channel $release_version${tag:+ ($tag)}; Windows $windows_arches, macOS $macos, at $SHA." >> "$GITHUB_STEP_SUMMARY"
