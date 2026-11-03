#!/bin/sh
# Prints the CHANGELOG section of one version: the text between "## [x.y.z]" and the next "## [".
# Usage: release-notes.sh <version without the v> [changelog path, default CHANGELOG.md]
set -eu

version="${1:?usage: release-notes.sh <version> [changelog]}"
changelog="${2:-CHANGELOG.md}"

notes=$(awk -v heading="## [$version]" '
  index($0, heading) == 1 { found = 1; next }
  found && index($0, "## [") == 1 { exit }
  found { print }
' "$changelog")

# A section with nothing but blank lines is as good as missing.
if [ -z "$(printf '%s' "$notes" | tr -d '[:space:]')" ]; then
  echo "release-notes.sh: no notes for $version in $changelog" >&2
  exit 1
fi
printf '%s\n' "$notes"
