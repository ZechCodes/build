#!/bin/sh
# Initialize a sample repo for the bridge to operate on (first run only), then
# start serving. In a real deployment BRIDGE_REPO is the user's actual project.
set -e

git config --global user.email "bridge@build.ing"
git config --global user.name "Build Bridge"
git config --global init.defaultBranch main

REPO="${BRIDGE_REPO:-/repo}"
if [ ! -d "$REPO/.git" ]; then
  mkdir -p "$REPO"
  cd "$REPO"
  git init -q -b main
  printf '# Sample project\n\nManaged by Build.\n' > README.md
  git add -A
  git commit -qm "initial"
fi
mkdir -p "${BRIDGE_WORKTREES:-/worktrees}"

exec build-bridge serve
