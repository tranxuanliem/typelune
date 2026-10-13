#!/usr/bin/env bash
# Print GitHub release notes for the current Cargo version: the CHANGELOG section plus an
# explicit signing/notarization notice. Used by .github/workflows/release.yml.
set -euo pipefail
cd "$(dirname "$0")/.."
eval "$(python3 scripts/product.py)"
section="$(awk -v v="$VERSION" '
  $0 ~ "^## \\[" v "\\]" {on=1; next}
  on && /^## \[/ {exit}
  on {print}' CHANGELOG.md)"
[ -n "$section" ] || { echo "CHANGELOG.md has no section for $VERSION" >&2; exit 1; }
cat <<NOTES
> **Developer preview — not notarized.** This build is ad-hoc signed only. It is not
> signed with an Apple Developer ID and has not been notarized by Apple, so macOS
> Gatekeeper will warn before opening it. If you prefer, build from source (see README).
> System-wide expansion is off by default and needs Accessibility permission, which you
> grant yourself.

Requires macOS $MINIMUM_MACOS or later on Apple Silicon. Verify the download with
\`SHA256SUMS.txt\`$( [ "${ATTESTED:-0}" = 1 ] && echo " and the build-provenance attestation (\`gh attestation verify\`)").
$section
NOTES
