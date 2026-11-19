#!/bin/bash
# A second version signed by the selected identity must satisfy the first version's requirement.
set -euo pipefail
app="${1:?usage: signing.sh Voai.app signing-identity}"
identity="${2:?specify the identity used for the local build}"
requirement="$(codesign -d -r- "$app" 2>&1 | sed -n 's/^.*designated => //p')"
case "$requirement" in
    ""|cdhash*) echo "The app has no persistent signing identity." >&2; exit 1 ;;
esac
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
updated="$stage/Voai.app"
ditto "$app" "$updated"
/usr/libexec/PlistBuddy -c 'Set :CFBundleVersion 999999' "$updated/Contents/Info.plist"
codesign --force --sign "$identity" --options runtime --timestamp=none --preserve-metadata=entitlements "$updated"
codesign --verify --strict --deep "-R=$requirement" "$updated"
