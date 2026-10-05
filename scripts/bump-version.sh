#!/usr/bin/env bash
# Set the version of all three packages (crate, wheel, npm) and start its
# CHANGELOG section. Usage: scripts/bump-version.sh 0.2.0
set -euo pipefail
cd "$(dirname "$0")/.."

version="${1:?usage: scripts/bump-version.sh <version, e.g. 0.2.0 or 0.2.0-rc.1>}"
if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]]; then
  echo "not a semver version: $version" >&2
  exit 1
fi

# The workspace version (the only line-leading `version =` in Cargo.toml).
perl -0pi -e "s/^version = \"[^\"]*\"/version = \"$version\"/m" Cargo.toml
# The npm package (the wheel reads its version from the workspace).
perl -0pi -e "s/\"version\": \"[^\"]*\"/\"version\": \"$version\"/" packages/lefsim/package.json
# Record the new workspace versions in Cargo.lock, without updating anything else.
cargo metadata --format-version 1 >/dev/null

# Turn "Unreleased" into this version's section, keeping an empty one on top.
if ! grep -q "^## $version " CHANGELOG.md; then
  perl -0pi -e "s/^## Unreleased\n/## Unreleased\n\n## $version - $(date +%Y-%m-%d)\n/m" CHANGELOG.md
fi

scripts/check-version.sh
echo "Next: review CHANGELOG.md, then commit, tag v$version and push the tag."
