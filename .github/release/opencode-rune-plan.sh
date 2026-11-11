#!/usr/bin/env bash
# opencode-rune-plan.sh: decides what .github/workflows/opencode-rune-release.yml builds and
# publishes from the event and ref alone, plus the pinned Browser Engine version and SHA-256.
#
#   pr-check  pull_request to main: builds and smoke-tests OpenCode (Rune).app; no signing/publish.
#   dev       the schedule (or dispatch dev on main): 0.0.0-dev-<run> on opencode-rune/desktop/dev
#             plus browser-engine/stable sync. SCOPE x64 (hourly) builds Windows x64; all
#             (nightly) also Windows arm64 and macOS. The dev feed carries Windows x64 only (see
#             FEED below). A scheduled run builds nothing when main hasn't changed an app file
#             since the last successful dev run of its scope.
#   check     dispatch check on main: builds, signs (when secrets exist), and verifies; no publish.
#   stable    tag opencode-rune-v<version> (or dispatch stable on that tag): publishes <version>
#             on opencode-rune/desktop/stable, every platform.
#   engine    tag browser-engine-<version>: publishes only opencode-rune/browser-engine/stable.
#
# Besides the release's identity it outputs windows-arches (JSON: the Windows builds), macos
# (whether macOS builds), feed-windows-arches and feed-macos (what is signed, verified and
# published), and publish-engine (only when the pinned engine isn't live yet).
. "$(dirname "$0")/lib.sh"

crate_version=$(awk '/^\[package\]/ { package = 1; next } /^\[/ { package = 0 } package && /^version = "/ { gsub(/^version = "|"$/, ""); print; exit }' apps/desktop/Cargo.toml)
[[ "$crate_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "Expected X.Y.Z version in apps/desktop/Cargo.toml, found $crate_version."

engine_version=$(sed -n 's/^pub const ENGINE_VERSION: &str = "\([^"]*\)";/\1/p' crates/browser-pane/src/installer.rs | head -1)
engine_release_version=$(printf '%s' "$engine_version" | sed -E 's/^([0-9]+\.[0-9]+\.[0-9]+)\.(.+)$/\1-\2/')
# The macOS SHA-256 (the first after ENGINE_DEFAULT_SHA256).
# TODO(IMPORTANT before release): build + sign (Authenticode) the Windows engine in CI with package-release.ps1, upload to the release CDN, then set ENGINE_DEFAULT_URL/SHA256 here.
# (Then read the windows-x64 SHA-256 here too and publish it as its own target.)
engine_sha256=$(awk '/ENGINE_DEFAULT_SHA256/ { want = 1 } want && match($0, /[0-9a-fA-F]{64}/) { print tolower(substr($0, RSTART, RLENGTH)); exit }' crates/browser-pane/src/installer.rs)
[[ "$engine_version" =~ ^[0-9]+\.[0-9]+ ]] || die "Could not parse ENGINE_VERSION from crates/browser-pane/src/installer.rs."
[[ "$engine_sha256" =~ ^[0-9a-f]{64}$ ]] || die "Could not parse ENGINE_DEFAULT_SHA256 from crates/browser-pane/src/installer.rs."

build_desktop=true
publish_engine=false
tag=""
scope=${SCOPE:-all}

if [ "$EVENT" = pull_request ]; then
  channel=pr-check
  release_channel=check
  release_version="0.0.0-dev-$GITHUB_RUN_NUMBER"
  notes="https://github.com/$GITHUB_REPOSITORY/commit/$SHA"
elif [ "$REF_TYPE" = tag ] && [[ "$REF_NAME" == browser-engine-* ]]; then
  tag_ver=${REF_NAME#browser-engine-}
  [ "$tag_ver" = "$engine_version" ] || die "Tag $REF_NAME does not match ENGINE_VERSION $engine_version."
  channel=engine
  release_channel=stable
  release_version="$crate_version"
  build_desktop=false
  publish_engine=true
  notes="https://github.com/$GITHUB_REPOSITORY/releases/tag/$REF_NAME"
else
  case "$CHANNEL" in
    stable)
      expected_tag="opencode-rune-v$crate_version"
      if [ "$REF_TYPE" = tag ] && [ "$REF_NAME" != "$expected_tag" ]; then
        die "Tag $REF_NAME does not match apps/desktop/Cargo.toml version $crate_version."
      fi
      if [ "$EVENT" = workflow_dispatch ] && [ "$INPUT_REF" != "$expected_tag" ]; then
        die "Stable dispatch must target tag $expected_tag, not $INPUT_REF."
      fi
      channel=stable
      release_channel=stable
      release_version="$crate_version"
      tag="$expected_tag"
      publish_engine=true
      notes="https://github.com/$GITHUB_REPOSITORY/releases/tag/$expected_tag"
      ;;
    dev)
      [ "$REF_NAME" = main ] || die "The dev channel runs from main, not $REF_NAME."
      if [ "$EVENT" = workflow_dispatch ] && [ "$INPUT_REF" != main ]; then
        die "The dev channel builds ref main, not $INPUT_REF."
      fi
      channel=dev
      release_channel=dev
      release_version="0.0.0-dev-$GITHUB_RUN_NUMBER"
      publish_engine=true
      notes="https://github.com/$GITHUB_REPOSITORY/commit/$SHA"
      ;;
    check)
      channel=check
      release_channel=dev
      release_version="0.0.0-dev-$GITHUB_RUN_NUMBER"
      notes="https://github.com/$GITHUB_REPOSITORY/commit/$SHA"
      ;;
    *)
      die "Unknown channel $CHANNEL."
      ;;
  esac
fi

case "$scope" in x64 | all) ;; *) die "Unknown scope $scope." ;; esac
[ "$channel" != stable ] || scope=all

# A scheduled dev run builds only when main changed an app file since the last successful dev
# run of its scope (an every-platform run counts for the x64 scope too). A run that finds nothing
# succeeds at the same commit, so it is the next one's base.
reason=""
if [ "$EVENT" = schedule ] && [ "$channel" = dev ]; then
  if [ "$scope" = all ]; then prefix="dev: every platform"; else prefix="dev: "; fi
  last=$(gh api "repos/$GITHUB_REPOSITORY/actions/workflows/opencode-rune-release.yml/runs?branch=main&status=success&per_page=100" \
    --jq "[.workflow_runs[] | select(.id != $GITHUB_RUN_ID and (.display_title | startswith(\"$prefix\")))][0].head_sha // empty" || true)
  if [ "$last" = "$SHA" ]; then
    reason="main is still at ${SHA:0:9}, which the last successful dev run of this scope built."
  elif [ -n "$last" ]; then
    files=$(gh api "repos/$GITHUB_REPOSITORY/compare/$last...$SHA" --jq '.files[]?.filename' || true)
    count=$(grep -c . <<< "$files" || true)
    app='^(apps/desktop/|apps/installer/|crates/|plugins/|Cargo\.toml$|Cargo\.lock$|rust-toolchain\.toml$|\.cargo/|\.github/workflows/opencode-rune-release\.yml$|\.github/release/)'
    # The compare API lists at most 300 files: more is a change worth a build.
    if [ "$count" -gt 0 ] && [ "$count" -lt 300 ] && ! grep -Eq "$app" <<< "$files"; then
      reason="no app file changed between ${last:0:9} and ${SHA:0:9}."
    fi
  fi
  if [ -n "$reason" ]; then
    build_desktop=false
    publish_engine=false
  fi
fi

# The pinned engine is published once: when it's live already, neither engine job starts.
if [ "$publish_engine" = true ]; then
  engine_zip="OpenCode-Browser-Engine-$engine_version-macos-arm64.zip"
  if manifest=$(curl -fsS "https://anoma.ly/release/opencode-rune/browser-engine/stable.json" 2>/dev/null) &&
    [ "$(printf '%s' "$manifest" | jq -r '.release.version // empty')" = "$engine_release_version" ] &&
    [ "$(printf '%s' "$manifest" | jq -r '.release.targets["macos-arm64"].sha256 // empty')" = "$engine_sha256" ] &&
    curl -fsSI "https://anoma.ly/release/opencode-rune/files/$engine_sha256/$engine_zip" >/dev/null 2>&1; then
    publish_engine=false
  fi
fi

if [ "$scope" = all ]; then windows_arches='["x64","arm64"]' macos=true; else windows_arches='["x64"]' macos=false; fi
# FEED: a channel's release is replaced whole, so an hourly x64 release drops the other
# platforms' targets; carrying their old archives forward under the new version would make
# those builds install the same archive at every check (the updater compares versions until an
# update has recorded the archive's SHA-256, updater/protocol.rs `decide`). So the dev feed
# carries Windows x64 alone: arm64 and macOS build nightly to show they still build (the
# unsigned arm64 setup goes to the rolling pre-release), and only stable, and check rehearsing
# it, signs and publishes them.
if [ "$channel" = dev ]; then
  feed_windows_arches='["x64"]' feed_macos=false
else
  feed_windows_arches=$windows_arches feed_macos=$macos
fi

{
  echo "sha=$SHA"
  echo "channel=$channel"
  echo "service=https://anoma.ly/release"
  echo "tag=$tag"
  echo "notes-url=$notes"
  echo "release-channel=$release_channel"
  echo "release-version=$release_version"
  echo "build-desktop=$build_desktop"
  echo "windows-arches=$windows_arches"
  echo "macos=$macos"
  echo "feed-windows-arches=$feed_windows_arches"
  echo "feed-macos=$feed_macos"
  echo "publish-engine=$publish_engine"
  echo "engine-version=$engine_version"
  echo "engine-release-version=$engine_release_version"
  echo "engine-sha256=$engine_sha256"
} >> "$GITHUB_OUTPUT"
if [ -n "$reason" ]; then
  echo "Nothing to build: $reason" >> "$GITHUB_STEP_SUMMARY"
else
  echo "Planned OpenCode (Rune) ($channel, $scope): desktop=$build_desktop ($release_channel $release_version; Windows $windows_arches, macOS $macos; signed and published: Windows $feed_windows_arches, macOS $feed_macos), engine=$publish_engine ($engine_version) at $SHA." >> "$GITHUB_STEP_SUMMARY"
fi
