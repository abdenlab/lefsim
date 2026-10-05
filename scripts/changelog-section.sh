#!/usr/bin/env bash
# Print one version's section of CHANGELOG.md (without its heading), for the
# GitHub release notes. Usage: scripts/changelog-section.sh 0.2.0
set -euo pipefail
cd "$(dirname "$0")/.."
awk -v v="$1" '
  /^## / { if (found) exit; if ($2 == v) { found = 1; next } }
  found { print }
' CHANGELOG.md
