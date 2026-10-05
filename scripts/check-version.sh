#!/usr/bin/env bash
# Check that the crate and the npm package agree on the version, and, given a
# tag (e.g. v0.2.0), that it names that version. CI runs this on every push;
# the release workflow runs it with the tag being released.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo_version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
npm_version=$(node -p 'require("./packages/lefsim/package.json").version')

if [[ "$cargo_version" != "$npm_version" ]]; then
  echo "version mismatch: Cargo.toml has $cargo_version, packages/lefsim/package.json has $npm_version" >&2
  exit 1
fi
if [[ $# -gt 0 && "$1" != "v$cargo_version" ]]; then
  echo "tag $1 doesn't match the version in Cargo.toml ($cargo_version)" >&2
  exit 1
fi
echo "version $cargo_version"
