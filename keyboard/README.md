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

Everything else is HeliBoard as released. HeliBoard's own clipboard-history
setting does not affect the sync.

Build: `./build.sh`. It clones the pinned tag into `upstream/`, applies the
patch, runs Gradle and copies `phonectl-keyboard.apk` into this directory.

Licence: this directory (patch, script, README) is GPL-3.0-only, like
HeliBoard. The rest of the phonectl repo stays MIT. The two apps only talk
through broadcasts.
