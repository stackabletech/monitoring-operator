#!/usr/bin/env sh
#
# Usage: perform.sh <level>
#
# <level> : "major", "minor" or "patch". Default: "minor".
#
BASE_BRANCH="main"
REPOSITORY="origin"
NOW_UTC=$(date -u '+%Y%m%d%H%M%S')
RELEASE_BRANCH="release-$NOW_UTC"

ensure_cargo_release() {
  cargo help release > /dev/null 2>&1 
  if [ "$?" != "0" ]; then
    >&2 echo "ERROR cargo-release not installed. Stop."
    exit 1
  fi
}

ensure_release_branch() {
  local STATUS=$(git status -s | grep -v '??')

  if [ "$STATUS" != "" ]; then
    >&2 echo "ERROR Dirty working copy found! Stop."
    exit 1
  fi

  git switch -c ${RELEASE_BRANCH} ${BASE_BRANCH}
  git push -u ${REPOSITORY} ${RELEASE_BRANCH}
}

tag_release_commit() {
  local TAG=$1
  git tag -a $TAG -m "release $TAG" HEAD^
  git push ${REPOSITORY} ${RELEASE_BRANCH} 
  git push --tags
}

maybe_create_github_pr() {
  local TAG=$1
  GH_COMMAND=$(which gh)
  if [ "$GH_COMMAND" != "" ]; then
    gh pr create --base $BASE_BRANCH --head $RELEASE_BRANCH --reviewer "@stackabletech/rust-developers" --title "Release $TAG" --body "Release $TAG"
  fi
}

main() {

  local RELEASE_LEVEL=${1:-minor}

  ensure_cargo_release

  ensure_release_branch

  cargo release ${RELEASE_LEVEL} --workspace --no-confirm

  local TAG_NAME=$(cat VERSION)

  tag_release_commit $TAG_NAME

  maybe_create_github_pr $TAG_NAME
}

main $@
