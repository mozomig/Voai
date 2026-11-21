#!/bin/bash
# Run the input-method UX probe in a disposable Zed document.
set -euo pipefail
cd "$(dirname "$0")/../.."
app="$HOME/Library/Input Methods/Voai Input Probe.app"
if [[ ! -x "$app/Contents/MacOS/Probe" || experiments/input-method/Probe.m -nt "$app/Contents/MacOS/Probe" ]]; then
    ./experiments/input-method/install
fi
work=$(mktemp -d)
file="$work/Voai IMK ${work##*/}.txt"
printf '%s\n' "${file##*/}" > "$file"
zed=/Applications/Zed.app/Contents/MacOS/cli
xcrun clang -fobjc-arc -framework Cocoa -framework Carbon -framework InputMethodKit experiments/input-method/Probe.m -o "$work/control"
open -g "$app"
"$zed" -n "$file"
"$work/control" demo "${file##*/}"
