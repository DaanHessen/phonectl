# phonectl redesign, 2026-11

Goal (owner's backlog): the phone link is always connected, keyboard sync works,
and the clipboard syncs both ways with no manual steps. It works over Tailscale
(phone on 4G/5G, laptop on any Wi-Fi), reconnects by itself after sleep,
network changes and reboots, mirrors every phone notification, and needs no
USB or wireless debugging in daily use (a payment app refuses to run while
debugging is on).

## 1. Why the link was down (diagnosis, 2026-11-11)

From the user journal (`journalctl _SYSTEMD_USER_UNIT=phonectl.service`, 28 Oct to 11 Nov):

1. **The binary was gone.** `~/.local/bin/phonectl` was a symlink into
   `target/release/`. Between the last session (2026-11-10 19:28, "phone
   stopped answering") and the next boot (19:29), `target/` was deleted,
   probably by a disk-space `cargo clean`. Since then the unit failed with
   `203/EXEC: Unable to locate executable` and restarted every 5 s: 5,765
   restarts by 12:35 the next day. The unit had no backoff and gave no
   visible sign of the failure. The phone app was not at fault: it was polling
   a laptop that was not listening.
2. **Phone → laptop clipboard practically never worked.** In two weeks the
   journal shows 1,029 `laptop → phone` clips but only one `phone → laptop`
   clip (2026-10-29). That path depended on watching logcat for
   ClipboardService denial lines. Android 13+ asks the user for log access
   ("Only this time") each time the app restarts, and declines silently when
   the app is in the background. After every reboot or app update the path
   stayed off until someone tapped a notification. This is exactly the
   "manual step" the owner wants gone.
3. **Gaps after a drop in Doze.** 15 sessions ended with "phone stopped
   answering". The next reconnect then took 18 to 62 minutes (for example
   10-28 14:55 → 15:45, 11-01 02:07 → 03:09, 11-09 17:08 → 17:54). The
   phone's retry timers run on the uptime clock, which stops in deep sleep,
   and the laptop only poked on startup, resume and network change. Laptop
   suspend gaps ("closed by laptop") are expected and reconnect within about
   20 s of resume.

Everything else worked on the real devices: Tailscale reconnects, Wi-Fi ↔ 5G,
notifications with actions, calls pausing media, and Waybar.

## 2. Options considered

| Option | Background clipboard read? | Survives reboot without a tap? | Debugging off in daily use? | Verdict |
|---|---|---|---|---|
| KDE Connect / GSConnect over Tailscale | only through READ_LOGS + floating activity (same as ours), or a Shizuku build | no (log-access prompt) | yes | No gain over phonectl. Discovery is UDP broadcast, so Tailscale needs manually added IPs. KDE bug 515707: 1.35.x fails the TLS handshake over Tailscale. |
| logcat watcher + invisible activity (current phonectl, KDE Connect) | yes, when armed | **no**: Android 13+ log-access consent per app start, declined in background | yes | Keep only as a fallback |
| `appops set … READ_CLIPBOARD allow` | no: the app-op is only checked *after* the focus/IME check | n/a | n/a | Does not work |
| `READ_CLIPBOARD_IN_BACKGROUND` | yes | yes | yes | signature/privileged only; `pm grant` cannot give it |
| Shizuku (shell-uid helper) | yes | **no**: non-root Shizuku must be restarted over wireless debugging after every reboot | no | Rejected |
| Accessibility service | no clipboard read; only guesses from UI events | yes | yes | Rejected: SoftPOS/payment apps flag active accessibility services, and Android 15 "restricted settings" makes it harder still |
| **Default input method (own keyboard)** | **yes, officially** ("unless your app is the default IME or has focus") | **yes**: the system binds the selected IME at boot | **yes**: no ADB at all | **Chosen** |

## 3. Chosen design

Keep the phonectl architecture (Rust daemon + Kotlin app, Tailscale TCP with
a Bluetooth fallback, HMAC handshake, event-driven reconnects). Fix the three
causes above, and add **phonectl Keyboard** for the clipboard and for typing.

```
 phone                                              laptop (Hyprland)
┌────────────────────────┐ explicit broadcasts ┌──────────────────────┐  Tailscale TCP  ┌─────────────────────┐
│ phonectl Keyboard      │  (signature perm.)  │ phonectl app         │◀───────────────▶│ phonectl daemon     │
│ HeliBoard + bridge     │── CLIP: text ──────▶│ NotificationListener │  (BT fallback)  │ wl-paste/wl-copy    │
│ default IME: may read  │◀─ TYPE: text/key ───│ Link, ClipSync,      │◀── UDP pokes ───│ freedesktop notifs  │
│ every clip, bound by   │                     │ status, calls        │                 │ Waybar, CLI, IPC    │
│ the system at boot     │                     └──────────────────────┘                 └─────────────────────┘
└────────────────────────┘
```

### Clipboard, phone → laptop: the keyboard

* `keyboard/` holds a patch on HeliBoard v4.2 (GPL-3.0, AOSP LatinIME
  lineage, actively maintained, real typing quality). The patch adds
  `PhonectlBridge.kt` and three hook lines. It does not fork the whole tree
  into this repo. `keyboard/build.sh` clones the pinned tag, applies the patch
  and builds.
* While it is the selected keyboard, `ClipboardService` dispatches every
  primary-clip change to it (the default IME passes `clipboardAccessAllowed`).
  The bridge reads the clip and drops anything marked
  `EXTRA_IS_SENSITIVE` (passwords, OTPs) or `EXTRA_IS_REMOTE_DEVICE` (our own
  writes, so no echo). It then sends the text to the phonectl app's
  `KeyboardBridge` receiver as an explicit broadcast guarded by
  `com.daanh.phonectl.permission.BRIDGE` (signature level, declared by both
  apps, both signed with the same key).
* `ClipSync` then does what it always did: hash dedupe, newest-wins after a
  reconnect, queue while offline.
* When the keyboard is active, the logcat watcher is not started at all: no
  consent prompt and no "tap to re-arm" notification. When another keyboard
  is selected, the old watcher remains as the fallback. The share sheet and
  Quick Settings tile still work as manual paths.

### Clipboard, laptop → phone

Unchanged. Any app may write the clipboard on Android, so the daemon's
`wl-paste --watch` sends each new laptop clip to the phone and the app sets
it. Password-manager clips (`x-kde-passwordManagerHint`) are not sent.

### Keyboard sync: type on the phone from the laptop

`phonectl type "text" [--key enter]` (or `phonectl type -` for stdin) sends a
`type_text` call. The app forwards it to the keyboard (TYPE broadcast, same
permission), which commits the text or presses the key in the focused field.
Keys: enter, newline, backspace, delete, tab, arrows, escape.

### Always connected

* **Binary:** `scripts/install.sh` builds in `~/.cache/phonectl-build` and
  *copies* the binary to `~/.local/bin/phonectl` (atomic replace). A
  `cargo clean` can no longer take the service down.
* **Unit:** `Restart=on-failure` with `RestartSteps=8` and
  `RestartMaxDelaySec=10min`. A broken install now backs off to one try every
  10 minutes instead of 17,000 a day, and recovers by itself once fixed.
* **Retry pokes (new):** while no phone is connected (and the laptop is awake,
  on Tailscale, with a known phone address), the daemon sends one
  authenticated UDP poke every 2 minutes for the first 10 minutes, then every
  5 minutes. That is one ~60-byte packet. It reaches a dozing phone through
  Tailscale's VPN service, and phonectl (battery: unrestricted, so exempt from
  Doze network limits) then takes its connect wakelock and dials. This closes
  the 18–62 minute gaps without the phone polling.
* Kept: the phone always dials, so Tailscale handles NAT and roaming. There
  are network callbacks on the phone, an rtnetlink watch plus resume pokes on
  the laptop, app-level ping/watchdog, the sleep notice before suspend, and
  the Bluetooth RFCOMM fallback.
* The phone process stays alive because the system binds its
  NotificationListenerService, as before. No foreground service is needed.
  The keyboard is likewise bound for as long as it is selected.

### Notifications

Unchanged and verified again: every posted/updated/removed phone
notification becomes a freedesktop notification (the owner's notification
center gets it over D-Bus). It shows the app name and icon, the notification
actions, "Open on phone", and dismissal both ways. Replying from the laptop is
still open (the notification center has no inline text input).

## 4. Reused vs replaced

| Part | Status |
|---|---|
| Rust daemon, link protocol, IPC, Waybar, notifications, media-on-call | reused |
| `phonectl type`, retry pokes, clipboard mode in `status` | new |
| `~/.local/bin/phonectl` symlink into `target/` | replaced by `scripts/install.sh` (copy) |
| systemd unit | backoff added |
| Android app: Link, NotifListener, StatusMonitor, Updater, ClipSync | reused; ClipSync gains keyboard mode, status reports `clipboard_mode` + `keyboard` |
| Android app: `Keyboard.kt` (bridge, `type_text` call), BRIDGE permission + receiver | new |
| logcat watcher + ClipReadActivity | kept as the fallback only |
| phonectl Keyboard (HeliBoard v4.2 + patch) | new, `keyboard/` |
| ADB stack (`crates/adb`), `phonectl setup` | one-time setup only, as before |

## 5. Costs and limits

* The owner has to use phonectl Keyboard as the daily keyboard. HeliBoard is
  a full keyboard: languages, emoji, clipboard history, themes. It has no
  cloud and no network permission of its own. Dutch needs its dictionary
  added once (HeliBoard settings → Languages → Dutch → add dictionary from the
  HeliBoard dictionaries page). Glide typing needs HeliBoard's optional
  gesture library, loaded once from its settings. If the owner switches back
  to Gboard, phone → laptop falls back to the logcat path or manual sharing.
* The keyboard and the app must be signed with the same key: the current
  debug keystore, SHA-256 `32:ba:e3:c6…3f:b6:3c`.
* Install the keyboard through the owner's phone store (Obtainium, a
  session-based installer). Android 13+ "restricted settings" only targets
  apps installed outside such installers, and IMEs are not on its documented
  list. If Android still says "restricted setting", use App info → ⋮ → Allow
  restricted settings.
* The retry pokes assume phonectl is battery-unrestricted on the phone (it
  asks for this on its setup screen) and that Tailscale is set as the
  always-on VPN.

## 6. One-time setup on the phone

None of this needs USB or wireless debugging; leave Developer options off.

1. In Obtainium (the owner's phone store), update **phonectl** to 0.2.0 and
   install **phonectl Keyboard**. Both are signed with the same key as the
   installed app, so phonectl updates in place and keeps its pairing.
2. Settings → System → Keyboard → On-screen keyboard (Manage keyboards) →
   turn on **phonectl Keyboard**. Android shows its standard warning that a
   keyboard can collect what you type; accept it. If Android says
   "Restricted setting": App info of phonectl Keyboard → ⋮ → Allow restricted
   settings, then try again.
3. Make it the keyboard in use: Settings → System → Keyboard → Default
   keyboard → phonectl Keyboard. Alternatively, tap the keyboard icon in the
   navigation bar while typing.
4. Optional: Dutch. Open phonectl Keyboard settings → Languages & layouts →
   add Nederlands, and load the Dutch dictionary from the link HeliBoard shows
   there. For glide typing, load the gesture library from Settings → Gesture
   typing (HeliBoard explains where to get it).
5. Open the phonectl app. "phonectl Keyboard", "Battery: unrestricted" and
   "Notification access" should all show ✓. Tap any ✗ to fix it.
6. Tailscale app: Android Settings → Network → VPN → Tailscale (gear) →
   Always-on VPN on. Set Tailscale's battery use to Unrestricted too.

Check on the laptop: `phonectl status` shows `clipboard  automatic both ways
(phonectl Keyboard)`. Copy something on the phone and paste it on the laptop.
`phonectl type "hello" --key enter` types into the focused field on the
phone.

## 7. Sources

* [AOSP ClipboardService.java](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/services/core/java/com/android/server/clipboard/ClipboardService.java): `clipboardAccessAllowed` allows READ_CLIPBOARD_IN_BACKGROUND holders, the default IME, the focused uid, content-capture and augmented-autofill services. Listeners only get `dispatchPrimaryClipChanged` after the same check. The app-op is noted *after* that check, so `appops … allow` cannot widen access.
* [Android 10 privacy changes](https://developer.android.com/about/versions/10/privacy/changes): "Unless your app is the default input method editor (IME) or is the app that currently has focus, your app cannot access clipboard data."
* [CTS: READ_CLIPBOARD_IN_BACKGROUND](https://android.googlesource.com/platform/cts/+/680b05b92af%5E%21/) and [/e/OS frameworks commits](https://gitlab.e.foundation/e/os/android_frameworks_base/-/commit/06ddc1fcc861c2db7c0afb7887ef20d18ea6ed28): the permission is signature-level (a role-grant was tried and reverted), so apps cannot get it.
* [KDE userbase: KDE Connect](https://userbase.kde.org/KDEConnect): auto clipboard on Android 10+ = `pm grant … READ_LOGS` + SYSTEM_ALERT_WINDOW (the method phonectl copied). Tunnels need devices added by IP.
* [KDE mailing list, 2023-11](https://mail.kde.org/pipermail/kdeconnect/2023-November/007744.html): how the log-watch + floating-window workaround works.
* [/e/OS LogcatManagerService commits](https://gitlab.e.foundation/e/os/android_frameworks_base/-/commit/e15e9f41c92db165b7bfe237a184106b41840a28): Android 13 log-access consent ("Only this time" / "Don't allow"), and background requesters are denied.
* [KDE bug 515707](https://bugs.kde.org/show_bug.cgi?id=515707): KDE Connect 1.35.x fails the TLS handshake over Tailscale.
* [Shizuku user manual](https://shizuku.rikka.app/guide/setup.html): non-root Shizuku must be started again over wireless debugging after every reboot.
* [Doze and App Standby](https://developer.android.com/training/monitoring-device-state/doze-standby): Doze suspends network access and ignores wakelocks. Apps on the battery-optimization exemption list "can use the network and hold partial wake locks during Doze". Timers and jobs are deferred.
* [Android 15 restricted settings (Android Authority)](https://androidauthority.com/android-15-restricted-settings-sideloading-3481098): restricted settings cover accessibility, notification listener and similar. Session-based installs are exempt.
* [HeliBoard](https://github.com/HeliBorg/HeliBoard): GPL-3.0 keyboard, v4.2 (2026-10-12). `ClipboardHistoryManager` already listens for clip changes, which is our hook point.
* phonectl's own journal, 2026-10-28 to 2026-11-11: the numbers in section 1.

## 8. Verification

* Laptop: `cargo test --release` passes 72 tests. The new daemon was run
  against `tests/fake_phone.py` (the real link protocol) on a private D-Bus
  with `tests/fake_notifyd.py` and the AI desktop's own Wayland clipboard,
  never the owner's. Checked: handshake, phone clip → laptop clipboard, laptop
  clip → phone (hash valid), notification with actions → freedesktop Notify,
  `phonectl type` text + key and stdin, `status` showing the clipboard mode,
  and a retry poke with a valid HMAC after the phone went away.
* Live: after `scripts/install.sh` the real service came up (13:01:56 on
  2026-11-11) and the phone, still on the old app, reconnected over Tailscale
  in the same second. The protocol is unchanged, so old app + new daemon work.
* Android app: `./gradlew test assembleRelease` passes (ProtocolTest 4/4).
  `app-release.apk` is signed with the same key as the installed app
  (`32bae3c6…`), and its manifest has the BRIDGE permission and receiver.
* Keyboard: see the report; building it needs approval to run HeliBoard's
  Gradle build here.
* Not yet verified on the real phone: anything that needs the new APKs
  installed.
