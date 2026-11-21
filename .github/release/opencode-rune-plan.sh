#!/usr/bin/env bash
# opencode-rune-plan.sh: decides what .github/workflows/release.yml builds and publishes, from the
# event, the ref, the repository's tags, and the live browser-engine index alone.
#
#   dev     the schedule (or a dispatch from dev): 0.0.0-dev-<run + 400> on the release
#           service's dev channel. Every release contains the full desktop matrix. A scheduled
#           run builds nothing when dev hasn't changed an app or engine file since the last
#           successful run and the live browser-engine index already serves this commit's engines.
#   check   a dispatch from dev: builds, signs and verifies all desktop targets and all four
#           browser engines, published nowhere. Its last job pushes and deletes a throwaway
#           v0.0.0-test-<run> tag to prove it can tag.
#   stable  a dispatch from dev with BUMP (patch, minor or major): the latest vX.Y.Z tag (0.0.0
#           when there is none) bumped, on the stable channel, every platform. Tags start nothing:
#           the workflow tags v<version> and creates the GitHub release only after publishing.
#   engine  a dispatch from dev: builds, signs, verifies and publishes the four browser engines
#           without building desktop; refuses if this commit's engine version is already complete
#           in the live index (bump ENGINE_REVISION or CEF to publish a new engine).
#
# A run releases the commit it runs on: SHA (the checked-out commit) must be GITHUB_SHA, so every
# job checks out github.sha.
#
# Environment: EVENT, REF_NAME, CHANNEL, BUMP, SHA, GITHUB_SHA, GITHUB_RUN_NUMBER,
# GITHUB_RUN_ID, GITHUB_REPOSITORY, GH_TOKEN.
. "$(dirname "$0")/lib.sh"

[ -n "$SHA" ] && [ "$SHA" = "${GITHUB_SHA:-}" ] || die "The checked-out commit $SHA is not this run's commit ${GITHUB_SHA:-}."
[ "$REF_NAME" = dev ] || die "Releases run from dev, not $REF_NAME."

release_dir=$(cd "$(dirname "$0")" && pwd)
build_desktop=true
tag=""
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
  engine)
    [ "$EVENT" = workflow_dispatch ] || die "Engine-only releases are dispatched from dev."
    build_desktop=false
    release_channel=stable release_version=""
    notes="https://github.com/$GITHUB_REPOSITORY/commit/$SHA"
    ;;
  *) die "Unknown channel $CHANNEL." ;;
esac

# Engine version from checked-out repository data (apps/browser-engine/Cargo.lock + installer.rs).
[ -f apps/browser-engine/Cargo.lock ] || die "Missing apps/browser-engine/Cargo.lock."
[ -f crates/browser-pane/src/installer.rs ] || die "Missing crates/browser-pane/src/installer.rs."
cef_build=$(grep -A1 '^name = "cef-dll-sys"$' apps/browser-engine/Cargo.lock |
  sed -n 's/^version = "[^"]*+\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\)"$/\1/p')
engine_revision=$(sed -n 's/^pub const ENGINE_REVISION: u32 = \([0-9][0-9]*\);$/\1/p' crates/browser-pane/src/installer.rs)
[[ "$cef_build" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "Could not parse CEF build from apps/browser-engine/Cargo.lock (got '$cef_build')."
[[ "$engine_revision" =~ ^[0-9]+$ ]] || die "Could not parse ENGINE_REVISION from crates/browser-pane/src/installer.rs (got '$engine_revision')."
engine_version="$cef_build.$engine_revision"
engine_release_version="$cef_build-$engine_revision"

# Query the live browser-engine index (fail hard on network, HTTP, or JSON errors).
engine_state=$(bash "$release_dir/opencode-rune-check-engine.sh" --plan-state --version "$engine_release_version")
engine_complete=$(sed -n 's/^complete=//p' <<< "$engine_state")
live_linux_sha256=$(sed -n 's/^linux_x64_sha256=//p' <<< "$engine_state")
[[ "$engine_complete" = true || "$engine_complete" = false ]] || die "Unexpected engine index state '$engine_complete'."
if [ "$engine_complete" = true ]; then
  [[ "$live_linux_sha256" =~ ^[0-9a-f]{64}$ ]] || die "Invalid linux-x64 sha256 '$live_linux_sha256' from engine index."
fi

# Only full-matrix runs from this workflow generation can serve as the comparison base.
reason=""
if [ "$EVENT" = schedule ] && [ "$CHANNEL" = dev ]; then
  prefix="dev: full desktop matrix"
  last=$(gh api "repos/$GITHUB_REPOSITORY/actions/workflows/release.yml/runs?branch=dev&status=success&per_page=100" \
    --jq "[.workflow_runs[] | select(.id != $GITHUB_RUN_ID and (.display_title | startswith(\"$prefix\")))][0].head_sha // empty" || true)
  if [ "$last" = "$SHA" ]; then
    reason="dev is still at ${SHA:0:9}."
  elif [ -n "$last" ]; then
    files=$(gh api "repos/$GITHUB_REPOSITORY/compare/$last...$SHA" --jq '.files[]?.filename' || true)
    count=$(grep -c . <<< "$files" || true)
    app='^(apps/desktop/|apps/installer/|apps/browser-engine/|crates/|features/|vendor/|plugins/|Cargo\.toml$|Cargo\.lock$|rust-toolchain\.toml$|\.cargo/|\.github/workflows/release\.yml$|\.github/release/)'
    # The compare API lists at most 300 files: more is a change worth a build.
    if [ "$count" -gt 0 ] && [ "$count" -lt 300 ] && ! grep -Eq "$app" <<< "$files"; then
      reason="no app or browser-engine file changed between ${last:0:9} and ${SHA:0:9}."
    fi
  fi
  [ -z "$reason" ] || build_desktop=false
fi

build_engine=false
linux_x64_sha256=""
case "$CHANNEL" in
  check)
    build_engine=true
    ;;
  engine)
    if [ "$engine_complete" = true ]; then
      die "Browser engine $engine_release_version is already complete in https://anoma.ly/release/opencode-rune/browser-engine/stable.json; bump ENGINE_REVISION (or CEF in apps/browser-engine/Cargo.lock) to publish a new engine."
    fi
    build_engine=true
    ;;
  dev | stable)
    if [ "$engine_complete" = false ]; then
      build_engine=true
      # A new Linux engine changes its archive hash, so desktop must rebuild to pin it.
      build_desktop=true
      reason=""
    else
      linux_x64_sha256=$live_linux_sha256
    fi
    ;;
esac

if [ "$build_engine" = false ] && [ "$build_desktop" = true ]; then
  [[ "$linux_x64_sha256" =~ ^[0-9a-f]{64}$ ]] || die "Missing valid linux-x64 sha256 for desktop build."
fi

{
  echo "channel=$CHANNEL"
  echo "service=https://anoma.ly/release"
  echo "tag=$tag"
  echo "notes-url=$notes"
  echo "release-channel=$release_channel"
  echo "release-version=$release_version"
  echo "build-desktop=$build_desktop"
  echo "engine-version=$engine_version"
  echo "engine-release-version=$engine_release_version"
  echo "build-engine=$build_engine"
  echo "linux-x64-sha256=$linux_x64_sha256"
} >> "$GITHUB_OUTPUT"
if [ "$build_desktop" = false ] && [ "$build_engine" = false ]; then
  echo "Nothing to build: $reason (browser engine $engine_release_version is already complete)." >> "$GITHUB_STEP_SUMMARY"
elif [ "$CHANNEL" = engine ]; then
  echo "Planned browser engine ($CHANNEL): $engine_release_version ($engine_version) at $SHA." >> "$GITHUB_STEP_SUMMARY"
else
  echo "Planned OpenCode (Rune) ($CHANNEL): $release_channel $release_version${tag:+ ($tag)}, engine $engine_release_version (build-engine=$build_engine), every platform at $SHA." >> "$GITHUB_STEP_SUMMARY"
fi
