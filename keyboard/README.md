# phonectl Keyboard

[HeliBoard](https://github.com/HeliBorg/HeliBoard) (GPL-3.0) with one small
addition, `PhonectlBridge.kt`. It sends the phone's clipboard to the laptop
without any manual step and lets the laptop type on the phone.

Why a keyboard: since Android 10, only the focused app and the **default
input method** may read the clipboard or be told it changed (AOSP
`ClipboardService.clipboardAccessAllowed`). A keyboard is the only way to get
every phone copy without ADB, log access, accessibility services or a tap
after each reboot. See `../docs/redesign-2026-11.md`.

What the patch changes (`patches/0001-phonectl-bridge.patch`):

* Package `com.daanh.phonectl.keyboard`, label "phonectl Keyboard", arm64
  only. The release build is signed with the debug key, the same key as the
  phonectl app.
* `PhonectlBridge.kt`: on every clipboard change, it sends the text to the
  phonectl app as an explicit broadcast guarded by the signature permission
  `com.daanh.phonectl.permission.BRIDGE`. It skips clips marked sensitive
  (passwords, OTPs) and clips that phonectl wrote itself. It also receives
  "type" requests from phonectl and types the text or presses the key in the
  focused field.
* Two hook lines in `LatinIME.java` and one in `ClipboardHistoryManager.kt`.
* HeliBoard's crash handler (`DebugFlags.kt`) also hands each crash report
  (versions and stack trace) to phonectl, which sends it to the laptop.
* The labels come from the string resource `phonectl_keyboard_name`
  (`res/values/phonectl.xml`). It must be a resource, not a literal: the
  setup wizard reads `applicationInfo.labelRes` and crashes on 0.
* The two content provider authorities are renamed to
  `com.daanh.phonectl.keyboard.*`, so stock HeliBoard can be installed next
  to it.

* Gboard-like defaults, measured from Gboard 16.0 on the owner's phone
  (Nothing A069P, 1260x2800, 440 dpi): keyboard 242dp plus a 46dp strip
  (924 px IME with the system's 48dp nav area, the same as Gboard),
  46.9dp keys, 12dp row gap, about 6dp key gap, 6dp corners, a pill enter
  key, flat keys with no bevel, letters at 50% of the key height, number
  hints on the top row only. Rounded style with key backgrounds and
  Material You colours (light/dark follow the system; at night the board
  is darker than the function keys, which are darker than the letters).
  Emoji key on the bottom row; the toolbar shows when nothing is typed
  (clipboard, emoji, undo, redo, select all, settings, voice). One
  English (UK) layout that also types Dutch.
* `AppUpgrade.kt`: on the first update to a build with a new look version
  (`phonectl_look`), the look and size prefs are reset once so existing
  installs get these defaults. Languages and other prefs are kept.

Everything else is HeliBoard as released. HeliBoard's own clipboard-history
setting does not affect the sync.

Build: `./build.sh`. It clones the pinned tag into `upstream/`, applies the
patch, runs Gradle and copies `phonectl-keyboard.apk` into this directory.

Licence: this directory (patch, script, README) is GPL-3.0-only, like
HeliBoard. The rest of the phonectl repo stays MIT. The two apps only talk
through broadcasts.
