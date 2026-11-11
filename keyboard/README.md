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

* Emoji search (version 4.2.6), inside the keyboard like Gboard. HeliBoard's
  own search is a separate activity that hides the keyboard and hands the
  emoji back after its exit transition (about a second), so the search key
  now starts `InlineEmojiSearch.kt` instead: the strip grows to two rows (a
  search field and a row of matching emojis; recent emojis when the field
  is empty), the letter keys type into the field (nothing reaches the app,
  swipe typing is off), and tapping an emoji commits it straight to the
  app's field while the keyboard stays open. The emoji key, the field's
  close button, or leaving the field ends the search; the query lives in
  memory only. Matching uses the bundled CLDR emoji dictionaries for the
  current language plus English and Dutch (`assets/dicts/emoji_en.dict`,
  `emoji_nl.dict`, copied into the build by `build.sh`), so "heart", "laugh",
  "hart" and "lachen" all work. Lookups run on a worker thread. The insert
  time is logged (`emoji search insert took N ms`, tag LatinIME). Data: CLDR
  emoji annotations and Signal's emoji keywords, compiled by Helium314 into
  <https://codeberg.org/Helium314/aosp-dictionaries> (`emoji_cldr_signal_dictionaries`).
  Licences: Unicode License v3 (CLDR) and AGPL-3.0 (Signal); both are
  compatible with GPL-3.0.
* Vibrate on keypress is on by default (system default duration) for fresh
  installs. Updates leave the existing vibrate setting and duration as they are.
* Suggestion strip: the right edge reserves the width of the toolbar key on
  the left, so suggestions are centred; the clipboard chip centres its text
  between the left padding and the close button.

* Version 4.2.7: the emoji search covers the strip with the keyboard
  background and hides the toolbar under it (before, the toolbar showed
  through the search row), and the search caret sits right after the query.
  The clipboard chip shows a one-line preview (`ClipPreview.kt`): links
  without `https://`, cut in the middle so the domain and the end stay
  visible; commands and code in monospace with line breaks shown as a return
  sign; control and bidi characters removed and the text cut on a code point
  boundary. The emoji panel opens on the recent emojis when there are any,
  and clipboard history keeps items for an hour (Gboard's default).
  `ImeNavBarFill.kt` paints the navigation bar area under the keyboard in the
  keyboard colour: apps targeting Android 15+ cannot colour it with
  `Window.setNavigationBarColor` any more, so on some phones and navigation
  modes the app behind showed through that band (HeliBoard issue 1818).

* Version 4.2.8: keyboard, emoji panel and emoji search are one surface.
  The strip (toolbar or suggestions) and the keyboard's own bottom row stay
  in place; the emoji panel only covers the letter rows above the bottom
  row, at exactly their height (`KeyboardView.setCoverBottom`). The panel's
  top row holds the search button, the category tabs and a delete key that
  repeats while held; the emoji key in the bottom row toggles back to the
  letters. Emoji search lives in the strip row (field on the left, matching
  emojis on the right), so the keyboard keeps its height there too.

* Version 4.2.9: leaving emoji search is one step. Tapping an emoji inserts
  it and closes the search (field cleared, letter keyboard back). The ABC
  and ?123 keys and the emoji key close the search (the emoji key only
  closes it, it does not open the panel). The system back button closes
  the search and keeps the keyboard open. Hiding the keyboard ends the
  search, so it always comes back as the letter keyboard.

Everything else is HeliBoard as released. HeliBoard's own clipboard-history
setting does not affect the sync.

Build: `./build.sh`. It clones the pinned tag into `upstream/`, applies the
patch, runs Gradle and copies `phonectl-keyboard.apk` into this directory.

Licence: this directory (patch, script, README) is GPL-3.0-only, like
HeliBoard. The rest of the phonectl repo stays MIT. The two apps only talk
through broadcasts.
