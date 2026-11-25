#!/usr/bin/env bash
# tag-release.sh: pushes the lightweight tag TAG at the released commit SHA (the checked-out HEAD)
# with the job's GITHUB_TOKEN, which the checkout left in git's config. release.yml runs it last,
# after every platform published, so a failed release leaves no tag; tags start nothing. A rerun
# accepts the tag when it already names SHA. A check run (TAG_TEST=1) pushes a throwaway tag and
# deletes it again. Runs git only, with hooks off.
. "$(dirname "$0")/lib.sh"

[[ "$TAG" =~ ^v0\.0\.0-(dev|test)-[0-9]+$ ]] || die "Unexpected tag $TAG."
[ "$(git rev-parse HEAD)" = "$SHA" ] || die "The checkout is not the released commit $SHA."
git() { command git -c core.hooksPath=/dev/null "$@"; }

existing=$(git ls-remote origin "refs/tags/$TAG" | cut -f1)
if [ -n "$existing" ]; then
  [ "$existing" = "$SHA" ] || die "Tag $TAG already names $existing, not $SHA."
  echo "Tag $TAG already names $SHA."
else
  git push -q origin "$SHA:refs/tags/$TAG"
  [ "$(git ls-remote origin "refs/tags/$TAG" | cut -f1)" = "$SHA" ] || die "Tag $TAG did not land."
  echo "Tagged $SHA as $TAG."
fi
if [ "${TAG_TEST:-}" = 1 ]; then
  git push -q origin ":refs/tags/$TAG"
  [ -z "$(git ls-remote origin "refs/tags/$TAG")" ] || die "Could not delete the test tag $TAG."
  echo "Deleted the test tag $TAG."
fi
