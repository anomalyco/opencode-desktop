#!/usr/bin/env bash
# publish-github-release.sh <dir>...: mirrors a stable release's signed files (every file in each
# <dir>) to the GitHub release of TAG, byte for byte, creating it with generated notes. release.yml
# runs it last, for stable only, after tag-release.sh tagged TAG at SHA; it never creates the tag.
#
# Environment: VERSION, TAG, SHA, GITHUB_REPOSITORY, GH_TOKEN.
. "$(dirname "$0")/lib.sh"

[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "Only a stable X.Y.Z release goes to GitHub, not $VERSION."
[ "$TAG" = "v$VERSION" ] || die "The release tag is $TAG, not v$VERSION."
[ $# -gt 0 ] || die "Usage: publish-github-release.sh <dir>..."
tagged=$(gh api "repos/$GITHUB_REPOSITORY/git/ref/tags/$TAG" --jq .object.sha)
[ "$tagged" = "$SHA" ] || die "Tag $TAG names $tagged, not $SHA."

if ! gh release view "$TAG" -R "$GITHUB_REPOSITORY" --json name > /dev/null 2>&1; then
  gh release create "$TAG" -R "$GITHUB_REPOSITORY" --verify-tag --title "OpenCode (Rune) $VERSION" \
    --generate-notes --latest=true
fi
mirror=$(mktemp -d)
trap 'rm -rf "$mirror"' EXIT
gh release view "$TAG" -R "$GITHUB_REPOSITORY" --json assets > "$mirror/release.json"
for dir in "$@"; do
  for file in "$dir"/*; do
    name=$(basename "$file")
    [ "$name" != SHA256SUMS ] || continue
    if ! jq -e --arg name "$name" '.assets | any(.name == $name)' "$mirror/release.json" > /dev/null; then
      gh release upload "$TAG" "$file" -R "$GITHUB_REPOSITORY"
    fi
    gh release download "$TAG" -R "$GITHUB_REPOSITORY" --pattern "$name" --dir "$mirror" --clobber
    cmp -s "$file" "$mirror/$name" || die "GitHub's $name differs from the release bytes."
    rm -f "$mirror/$name"
  done
done
echo "https://github.com/$GITHUB_REPOSITORY/releases/tag/$TAG" >> "$GITHUB_STEP_SUMMARY"
