#!/usr/bin/env bash
# Test helper (development only): copies TEXT on the phone *inside Chrome's
# address bar*, i.e. in another app, which exercises phonectl's background
# clipboard path. Leaves Chrome without navigating.
set -euo pipefail
text="$1"
adb shell am start -W -n com.android.chrome/com.google.android.apps.chrome.Main -d about:blank >/dev/null
sleep 2
adb shell 'input keycombination KEYCODE_CTRL_LEFT KEYCODE_L'
sleep 0.7
adb shell input text "$(printf '%s' "$text" | sed 's/ /%s/g')"
adb shell 'input keycombination KEYCODE_CTRL_LEFT KEYCODE_A; input keycombination KEYCODE_CTRL_LEFT KEYCODE_C'
sleep 0.3
adb shell input keyevent KEYCODE_ESCAPE
