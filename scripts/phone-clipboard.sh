#!/usr/bin/env bash
# Test helper (development only, needs ADB and an unlocked phone): prints the
# phone's current clipboard by pasting it into the phonectl app's pairing field
# and reading the field back with uiautomator. The field is cleared after.
set -euo pipefail
adb shell am start -W -n com.daanh.phonectl/.MainActivity >/dev/null
sleep 1
dump() { adb shell uiautomator dump /sdcard/phonectl-ui.xml >/dev/null && adb shell cat /sdcard/phonectl-ui.xml; }
bounds=$(dump | grep -o '<node[^>]*EditText[^>]*>' | head -1 | grep -o 'bounds="[^"]*"' | grep -o '[0-9]\+' | tr '\n' ' ')
read -r x1 y1 x2 y2 <<<"$bounds"
adb shell input tap $(( (x1 + x2) / 2 )) $(( (y1 + y2) / 2 ))
clear() { adb shell 'input keycombination KEYCODE_CTRL_LEFT KEYCODE_A; input keyevent KEYCODE_DEL'; }
clear
adb shell input keyevent 279   # KEYCODE_PASTE
sleep 0.5
dump | grep -o '<node[^>]*EditText[^>]*>' | head -1 | grep -o ' text="[^"]*"' | sed 's/^ text="//; s/"$//'
clear
adb shell input keyevent KEYCODE_BACK
adb shell rm -f /sdcard/phonectl-ui.xml
