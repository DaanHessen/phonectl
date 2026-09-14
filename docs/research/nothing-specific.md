# Nothing Phone 2 ("Pong") — What's Nothing-specific, and can phonectl reach it without root/app?

Scope: this covers only what is *specific to Nothing OS / the Nothing Phone 2 hardware* (Glyph
Interface, Nothing OS internals, Fast Pair, Essential Space/Glyph Progress). Battery,
notifications, and media are assumed to come from KDE Connect + ADB and are out of scope here.
Nothing Ear/earbud battery is out of scope (handled by the user's separate `earctl` over RFCOMM
directly to the earbuds) — noted only where it intersects with the phone side.

Confidence tags used below: **[verified]** = read directly in primary source code/docs by this
research pass, **[documented]** = stated in official Nothing docs/README, **[community]** =
claimed by a third-party project/XDA thread without independent verification here,
**[speculation]** = inference, not confirmed anywhere.

---

## 1. Glyph Interface

### 1.1 Official Glyph Developer Kit

- Repo: https://github.com/Nothing-Developer-Programme/Glyph-Developer-Kit (Phone 1/2/2a).
  Newer Phone 3 matrix version: https://github.com/Nothing-Developer-Programme/GlyphMatrix-Developer-Kit
- **[verified]** The real backing package is **`com.nothing.thirdparty`**, not
  `com.nothing.ketchum` (that name is only the *client SDK's Java package / permission
  namespace baked into the AAR that ships inside your own app — `com.nothing.ketchum.GlyphManager`,
  `com.nothing.ketchum.GlyphFrame`). The actual system-side service lives in the
  `com.nothing.thirdparty` app/process and is reached via:
  - Bind action: `com.nothing.thirdparty.bind_glyphservice`
  - Component: `com.nothing.thirdparty.GlyphService`
  - AIDL interface: `IGlyphService` with methods `register(String apiKey)`, `openSession()`,
    `setFrameColors(int[] values)`, `closeSession()`.
    Source: https://github.com/rec0de/glyph-api (independent reverse-engineering write-up of the
    "unofficial" wire protocol; cross-checked against the official kit's higher-level
    `GlyphManager` wrapper, which adds `toggle()`, `animate()`, `displayProgress()`,
    `displayProgressAndToggle()`, `turnOff()` on top of the same AIDL call).
- **[documented]** Manifest requirements for a client app:
  - Permission: `com.nothing.ketchum.permission.ENABLE`
  - `<meta-data android:name="NothingKey" android:value="test"/>` for development, or a real
    issued key for production.
  - `GlyphManager.getInstance(Context)` → `init(Callback)` in `onCreate` → on service-connected,
    call `register()` (or `register(String targetDevice)`) → `openSession()` before controlling
    lights → `closeSession()` when done. Only the **foreground app** is allowed to hold a session;
    backgrounding resets it unless the caller has special system privileges.
  - **[documented]** "API key restriction has been removed starting from Android B (Android 16)" —
    i.e. on a future Nothing OS build running Android 16, any app can call the Glyph API without a
    registered key. Phone 2 today runs Nothing OS 2.x/3.x on Android 14/15, so this does **not**
    currently apply, but it is a documented forward-looking relaxation worth tracking.
- **[verified via rec0de/glyph-api]** Authentication inside `com.nothing.thirdparty` is done by:
  1. SHA1 fingerprint check of the calling app's signing cert against a value derived from the
     API key, checked against a **remote allow-list** Nothing publishes for known package names.
  2. A hardcoded special-case for `com.nothing.glyph.composer` (Nothing's own Glyph Composer app).
  3. **Automatic approval for callers running as system UID (1000).**
  4. rec0de's write-up flags that the fingerprint check only validates a partial/truncated SHA1,
     which is a theoretical brute-forceable weakness for impersonating a registered app — not
     something to build on, but noted for completeness.
- **[verified via rec0de/glyph-api]** **Direct ADB/shell invocation is not possible.** The service
  is a normal bound Android service inside an app process, not a `ServiceManager`-registered
  system service, so there is no `adb shell service call ...` path to it. The shell UID (2000)
  has no registered package identity, no signing certificate, and no whitelisted name, so it
  cannot `bindService()` to it and would fail authentication even if it could. **Conclusion: you
  cannot drive the Glyph Interface with adb alone and zero app installed on the phone.**

### 1.2 Debug bypass toggle

- **[community, but standard/reliable]** `adb shell settings put global nt_glyph_interface_debug_enable 1`
  enables a debug mode that lets a locally-built/sideloaded app using the `"test"` API key call the
  Glyph service without a whitelisted key, and auto-disables after 48 hours. This is a `global`
  Settings key; writing global settings from **plain (non-root) `adb shell`** normally works
  because the `shell` UID on stock Android carries `WRITE_SECURE_SETTINGS`-class ADB privilege for
  most `global`/`secure` keys — this specific key is exactly the kind of thing Nothing's own kit
  instructs third-party devs to run from a **non-rooted** phone, so no root is implied here.
  Source citing this workflow: multiple Flutter/Dart Glyph wrapper packages (e.g.
  `nothing_glyph_interface` on pub.dev, `flutter-nothing-glyph-interface` on GitHub) instruct
  exactly this command as a prerequisite for testing without a production key.
- This toggle only relaxes the *key/whitelist* check — it does **not** let adb/shell itself become
  a valid caller. An actual installed app (even a trivial one) is still required to hold the
  bound session.

### 1.3 Phone (2) zone/channel layout

- **[documented, via Glyph Developer Kit + community indices]** Phone (2) exposes **33
  addressable zones** (indices 0–32): `A1`(0), `A2`(1), `B1`(2), `C1_1`..`C1_16`(3–18),
  `C2`..`C6`(19–23), `E1`(24), `D1_1`..`D1_8`(25–32). `setFrameColors(int[])` takes one intensity
  value per zone. The official kit's `GlyphFrame`/`GlyphFrame.Builder` wraps this into named
  channels per zone group. Exact per-zone indices are also tabulated by
  https://github.com/SebiAi/custom-nothing-glyph-tools (its docs directory has a per-model
  `glyphId` page; the specific file for Phone (2) exists in that repo under
  `docs/4_First Composition/1b_glyphId Nothing Phone (2).md`, though this pass could not fetch its
  raw contents directly — GitHub blob and raw URLs 404/403'd for this specific file during
  research; re-fetch on demand, or clone the repo, to pull the exact table).

### 1.4 Root-only sysfs path (not usable for phonectl's no-root goal)

- **[community — XDA]** Thread "Glyph Control Via Shell / Sysfs (Root Required)":
  https://xdaforums.com/t/glyph-control-via-shell-sysfs-root-required.4645837/
  Confirms (title + search snippet) that direct sysfs control (e.g.
  `echo 1 > /sys/class/leds/led_strips/operating_mode`, and presumably per-LED brightness files
  under an `aw210xx`/`aw20036`-family LED driver node) **requires root** (`su`), and recommends
  stopping/disabling Nothing's own service (`com.nothing.thirdparty`, referred to informally as
  "NtThirdParty") first to avoid the two fighting over the LEDs. Full page fetch was blocked by
  XDA's Cloudflare (403) during this research pass — if exact sysfs paths are needed, revisit via
  a browser session or Google cache, or better: read them directly off the physical phone (see
  §7, item 5) rather than trusting a forum mirror.
- The Phone 2 kernel LED driver module is `led_aw20036` **[community]**, consistent with the
  `aw210xx_led` class name mentioned in the task — i.e. `/sys/class/leds/aw210xx_led/...` or
  similarly-named nodes likely exist, but only shell UID *within a root context* can write them;
  plain shell/app UID is very unlikely to have write permission on kernel LED sysfs nodes (typical
  perms are `system`/`root` owned, mode 660 or 644 root-only for brightness attributes). This is
  **[speculation]** until verified against the actual `ls -la /sys/class/leds/` on the device —
  see the adb command list below.

### 1.5 Community non-root Glyph apps

- **Glyphify** (https://github.com/Fr4nKB/Glyphify-v1, https://glyphify.app/) — commercial,
  non-root Android app, standard user-installed APK using the official Glyph SDK (per-zone
  contact/app mapping, static/pulse patterns). Confirms the official SDK path works fine
  non-rooted, i.e. root is only needed for the raw sysfs bypass, not for the SDK itself.
  **[community]**
- **GlyphTones** / **glyphtorch-non-root** (https://github.com/aimok04/glyphtorch-non-root) —
  another non-root community app using the same official SDK route for a flashlight-style
  Glyph toggle. Reinforces: non-root Glyph control is only possible *from an installed Android
  app*, never from adb/PC directly. **[community]**
- **SebiAi/custom-nothing-glyph-tools** (https://github.com/SebiAi/custom-nothing-glyph-tools) —
  PC-side (Python) tooling that builds `.glyph`/`.ogg`-embedded "Glyph Composition" ringtone files
  for Nothing's own **Glyph Composer** app to import and play *on the phone*. This is the closest
  existing "PC talks to Nothing Glyph stuff" project, but it works by producing a file that must
  be transferred to the phone and opened in Nothing's app — it does not send live commands from a
  PC over adb. Good prior art for file format/tooling patterns, not for a live-control transport.

### 1.6 Minimal path if an app is required

Since no ADB-only or rootless-companion-app-free path exists, the minimal viable architecture for
`phonectl` to touch the Glyph Interface is:

1. Build/sideload a tiny companion APK (`phonectl-glyph-bridge` or similar) with:
   - The official Glyph SDK embedded, `NothingKey=test` + the debug settings toggle for
     development, or a real issued API key for a public release.
   - **One exported `BroadcastReceiver`** (or a bound service reachable via
     `adb shell am broadcast`) that translates a simple intent (extras: zone id, color, progress
     value, on/off) into `GlyphManager` calls. Because the app must be foregrounded to hold a
     session per the kit's rules, this likely needs a lightweight persistent
     foreground/accessibility-service style component, not just a receiver.
   - Triggered from the PC via `adb shell am broadcast -a com.phonectl.GLYPH_SET ...` (adb over
     USB or Wi-Fi, no root needed on the phone for this leg — root is not required to install a
     debug-signed APK and adb-broadcast into it).
2. This is the same shape of solution KDE Connect itself uses (a phone-side app receiving
   commands) — so `phonectl` would effectively need its own minimal KDE-Connect-plugin-like
   Android app just for Glyph, while everything else (battery, notifications, media) stays on
   stock KDE Connect.

---

## 2. Nothing OS internals visible from plain ADB (no root)

No pre-existing catalogue of Nothing-specific `getprop` keys, exported components, or content
providers for Nothing OS 2.x/3.x turned up in primary sources during this pass — Nothing does not
publish a build.prop reference, and no community writeup enumerates it comprehensively.
**[speculation/gap]** — this must be captured directly from the physical device (see §7).

What is confirmed from device-tree/dump sources:
- **[verified]** Nothing Phone 2 ("Pong") firmware carries at least these Nothing-namespaced
  feature-flag properties in `system_ext.prop` (from the LineageOS device tree for Pong):
  `ro.build.nothing.feature.base=0x4458438124a040126b4247b97ffL` and
  `ro.build.nothing.feature.diff.device.Pong=0x2001aa4144ac04803d1842000L`. These are opaque
  bitmask feature flags (format undocumented) — not directly actionable, but confirm the
  `ro.build.nothing.feature.*` namespace exists and is queryable with plain `getprop` (no root).
  Source: https://github.com/Nothing-phone-2-Development/android_device_nothing_Pong/blob/lineage-22.1/system_ext.prop
- Expect (not yet confirmed) additional `ro.nothing.*`/`ro.vendor.nothing.*` keys for hardware
  region/model variant, and Nothing's own package namespace `com.nothing.*` /
  `com.google.android.gms`-adjacent Fast-Pair provider strings. Full dump.tadiphone.dev mirror for
  a similar Nothing device ("spacewar") exists at
  https://dumps.tadiphone.dev/dumps/nothing/spacewar/-/blob/06054fc3f57fcd48f5e1c5004a0f40bdade3c7f3/vendor/build.prop
  and could be diffed offline if a Pong-specific dump surfaces later, but this is a *different*
  Nothing device (Phone 2a family, not confirmed to be Pong), so treat as directional only.

---

## 3. Nothing X / Nothing Ear interplay (informational only)

Out of scope per the task (user's `earctl` already talks to Nothing Ear directly over RFCOMM).
No phone-side Nothing API for earbud battery was found that would be preferable to that direct
path; Nothing X app battery display is itself sourced from the earbuds over the same Bluetooth
GATT/RFCOMM channel `earctl` already uses, so there's nothing Nothing-OS-specific to gain here.
**[speculation, low-stakes]**

---

## 4. Existing Linux/community projects for Nothing Phone

- `SebiAi/custom-nothing-glyph-tools` — PC-side composition tooling (see §1.5), Python, MIT-ish,
  actively maintained, good reference for `.glyph` binary/metadata format if `phonectl` ever
  wants to *build* compositions on the PC and push the resulting file to the phone for Glyph
  Composer to import (not live control).
- `rec0de/glyph-api` — the clearest technical breakdown of the underlying Binder/AIDL protocol;
  valuable for anyone who later needs to build the companion-APK bridge in §1.6.
- No project was found that achieves live Glyph control purely from a PC over adb without any
  on-phone app — consistent with the protocol-level finding in §1.1 that this is not possible as
  designed.
- Nothing OS custom-ROM/root community exists (`android_device_nothing_Pong` LineageOS device
  tree, XDA rooting guides) but is orthogonal to `phonectl`'s no-root goal.

---

## 5. Glyph Progress / notification-driven Glyph without a companion app

- **[documented]** "Glyph Progress" and "Live Updates" in current/upcoming Nothing OS piggyback on
  **standard Android notification features**: Nothing states any app using Android 16's
  **Progress-centric notifications / Live Updates API** will automatically surface on the Glyph
  Interface — i.e., this is Nothing OS reading a **standard system notification API surface**
  (progress-style notifications), not a Nothing-proprietary intent. This means, **on a future
  Nothing OS build running Android 16**, a normal Android notification with progress semantics —
  which is exactly the kind of thing KDE Connect or an ADB-pushed notification could produce —
  might light up the Glyph Interface with **zero Nothing-specific API calls and no companion
  app**, just a correctly-shaped standard notification.
- **Caveat:** Nothing Phone 2 is currently on Android 14/15 (Nothing OS 2.x/3.x), so this Live
  Updates path is **not yet available** on this hardware/OS combination as of today
  (2026-09-14) unless/until it receives an Android 16-based Nothing OS update. Treat this as a
  **near-term opportunity to re-test after any OS upgrade**, not something to build against now.
- "Essential Notifications" (assign an LED strip glow to an app/contact without SDK involvement)
  is a **user-configured OS feature**, not something a PC/adb can programmatically trigger or
  read — it only reacts to real notifications arriving on-device.
- No evidence found that standard Android `Notification` extras (without the Live Updates/progress
  API shape) trigger any Glyph behavior on today's OS version. **[speculation/gap]** — worth a
  direct experiment on-device (post a plain notification via `adb shell cmd notification post ...`
  and watch for any Glyph reaction) rather than trusting docs alone; see §7.

---

## 6. Fast Pair / companion-device relevance to a PC

- **[community/general]** Nothing Phone supports Google Fast Pair for *earbuds* pairing UX
  (BLE proximity trigger when opening an Ear case) — this is Google's generic Fast Pair, not a
  Nothing-specific extension, and it is a phone-as-scanner behavior, not something a Linux PC
  benefits from. Linux has no first-party Fast Pair stack; GNOME/KDE do not implement the Fast
  Pair BLE handshake. **Conclusion: no relevant PC-facing angle here — skip.**
- No Nothing-specific companion-device (Android "Companion Device Manager") integration relevant
  to PC control was found.

---

## 7. Read-only ADB commands to run on the physical phone to resolve remaining unknowns

All of these are non-destructive reads (or, where marked, a reversible debug-settings write that
Nothing's own kit instructs developers to use). Run with the phone unlocked and
`adb devices` already showing it authorized.

```sh
# --- Build/property surface: find every Nothing-namespaced prop ---
adb shell getprop | grep -i nothing
adb shell getprop | grep -iE 'ro\.build|ro\.product|ro\.vendor' | grep -i noth
adb shell getprop ro.build.nothing.feature.base
adb shell getprop ro.build.nothing.feature.diff.device.Pong

# --- Package inventory: everything under Nothing's namespace ---
adb shell pm list packages | grep -i nothing
adb shell pm list packages -f | grep -i nothing

# --- Glyph service package: confirm com.nothing.thirdparty exists, its exported surface ---
adb shell pm path com.nothing.thirdparty
adb shell dumpsys package com.nothing.thirdparty | grep -A3 -iE 'permission|service|receiver|provider'
adb shell dumpsys package com.nothing.glyph.composer | grep -A3 -iE 'permission|service|receiver|provider'

# --- Confirm the debug settings key currently reads as expected (read-only check) ---
adb shell settings get global nt_glyph_interface_debug_enable

# --- (Reversible) enable the documented debug toggle for later app-based testing, then re-check ---
# adb shell settings put global nt_glyph_interface_debug_enable 1
# adb shell settings get global nt_glyph_interface_debug_enable
# adb shell settings put global nt_glyph_interface_debug_enable 0   # revert when done

# --- Sysfs LED nodes: existence + ownership/permissions only, no writes ---
adb shell ls -la /sys/class/leds/
adb shell ls -la /sys/class/leds/aw210xx_led/ 2>/dev/null
adb shell ls -la /sys/class/leds/led_strips/ 2>/dev/null
adb shell cat /sys/class/leds/led_strips/operating_mode 2>/dev/null   # read, don't write
adb shell find /sys/class/leds -maxdepth 2 -name '*bright*' -exec ls -la {} \;

# --- Kernel LED driver module presence ---
adb shell lsmod | grep -iE 'aw2003|aw210|led_aw'
adb shell cat /proc/modules | grep -iE 'aw2003|aw210|led_aw'

# --- Any content providers Nothing exposes system-wide ---
adb shell dumpsys package providers | grep -i nothing

# --- Check whether shell UID has WRITE_SECURE_SETTINGS-class ability confirmed in practice ---
adb shell id
adb shell dumpsys package com.android.shell | grep -i permission

# --- Live-Updates / Glyph Progress reaction test (non-destructive; cancel after) ---
adb shell cmd notification post -S progress -p 50 phonectl_test "Test" "Progress test body"
# observe phone/Glyph, then:
adb shell cmd notification cancel phonectl_test

# --- OS/Android version actually running, to know if Android-16 Live Updates path applies ---
adb shell getprop ro.build.version.release
adb shell getprop ro.build.version.sdk
adb shell getprop ro.nothing.os.version 2>/dev/null
```

---

## 8. Bottom-line conclusions

1. **Glyph Interface cannot be controlled from a PC via adb alone, with no app on the phone.**
   The service is a normal bound Android service inside `com.nothing.thirdparty`, authenticated by
   signing-cert/API-key, with no `ServiceManager`-level entry point. **[verified]**
2. **Root is not required for Glyph control in general** — the official SDK plus the documented
   `nt_glyph_interface_debug_enable` settings toggle is the standard non-root dev path, used by
   real published non-root apps (Glyphify, glyphtorch-non-root). **[documented + community]**
   Root is only needed for the raw sysfs bypass (`/sys/class/leds/...`), which `phonectl` should
   not pursue given the no-root constraint. **[community]**
3. **Minimal viable path for phonectl:** ship a tiny companion Android app (debug-key or
   eventually a real registered key) with an adb-reachable broadcast/intent surface, bridging
   `adb shell am broadcast` commands from the PC into `GlyphManager` calls. This mirrors how KDE
   Connect itself works and is the only architecture that can work today.
4. **A companion-app-free path may open up later**: if/when the phone updates to an Android
   16-based Nothing OS, standard Android Live Updates/progress notifications are documented to
   surface automatically as Glyph Progress — worth re-testing after any future OS update, and
   worth a direct experiment now with `adb shell cmd notification post -S progress` per §7 in case
   partial behavior already exists on 14/15.
5. **Nothing OS internals** (getprop keys, exported components/providers) are not documented
   anywhere publicly in a consolidated way; they must be harvested directly off the physical device
   using the read-only command list in §7 — this is a gap this report could not close from
   secondary sources alone.
6. **Fast Pair has no useful PC-facing angle** for this project; skip it.
7. **Earbud battery**: no phone-side Nothing API is preferable to the existing direct-RFCOMM
   `earctl` approach — no action needed here.
