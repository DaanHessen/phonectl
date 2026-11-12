# How phone → laptop clipboard works without root or a custom keyboard

Android 10 and later only let three kinds of apps read the clipboard: the app
the user is looking at, the default keyboard, and holders of the signature
permission `READ_CLIPBOARD_IN_BACKGROUND`. A sync app is none of those, so
clipboard sync apps either ship their own keyboard, ask for a tap per copy,
use root, or run a helper through Shizuku that has to be restarted over
wireless debugging after every reboot.

phonectl borrows the permission from the shell user instead, with a test
mechanism Android has had since API 29: an instrumentation started from a
shell may adopt the shell's permissions for its own app. This page explains
how that works, why it survives the debugging connection ending, and what it
costs.

## The pieces

| Piece | Where | Role |
|---|---|---|
| `ShellWatcher` | `android/.../ShellWatcher.kt` | An `Instrumentation` declared in phonectl's own manifest, targeting phonectl itself |
| `ClipSync` | `android/.../ClipSync.kt` | phonectl's ordinary clipboard listener and sender (unchanged) |
| `AdbWindow` | `android/.../AdbWindow.kt` | Opens wireless debugging for a few seconds when the laptop needs to start the watcher |
| `daemon::watcher` | `crates/phonectl/src/daemon/watcher.rs` | Laptop side: notices the watcher is down and starts it |
| `phonectl setup-clipboard` | `crates/phonectl/src/setup.rs` | The one ADB session: pair, grant, first start |

## Step by step

1. `am instrument com.daanh.phonectl/.ShellWatcher` runs as the shell user
   (uid 2000) over ADB. ActivityManager force-stops phonectl, starts a fresh
   process for it with the instrumentation class loaded, and hands that
   process a `UiAutomationConnection` owned by the shell.
2. `ShellWatcher.onCreate` runs before `App.onCreate`; it marks the process as
   instrumented and calls `start()`, which runs `onStart` on its own thread.
3. `onStart` calls
   `getUiAutomation(FLAG_DONT_USE_ACCESSIBILITY).adoptShellPermissionIdentity(READ_CLIPBOARD_IN_BACKGROUND)`.
   From then on, permission checks for phonectl's uid that ask for exactly
   that permission are answered as if phonectl were the shell. No other
   permission is adopted. `FLAG_DONT_USE_ACCESSIBILITY` means no
   accessibility service is registered (banking apps that refuse to run next
   to one are not affected).
4. `ClipboardService.clipboardAccessAllowed` checks
   `checkPermission(READ_CLIPBOARD_IN_BACKGROUND, callingPackage)`; with the
   adopted identity it passes, so `ClipSync`'s normal
   `OnPrimaryClipChangedListener` is now called for every copy in any app,
   and `getPrimaryClip()` returns it. No polling.
5. `onStart` never returns, so the instrumentation stays active. Started
   without `-w`, it does not depend on the ADB connection: it keeps running
   after the shell exits and after wireless debugging is switched off (adbd
   restarts, but the process belongs to phonectl's uid, not to adbd's cgroup).
   `dumpsys activity processes` shows it as `mInstr=ActiveInstrumentation{…ShellWatcher}`.
6. When the watcher is up, the app reports it (`status.shell_watcher.running`)
   and switches wireless debugging off again if it was the one that switched
   it on.

A plain `app_process` helper started as shell would also have the
permission, but it lives in adbd's cgroup and dies when wireless debugging is
switched off. That is why the clipboard reader runs inside phonectl's own
process.

## Restarting it with no one touching the phone

The instrumentation ends when phonectl's process ends: a phone reboot, an app
update, a crash or a force stop. Then:

1. The app starts normally (boot receiver, update receiver) and connects to
   the laptop over Tailscale. Its status says `shell_watcher.running: false`,
   `can_toggle_adb: true`.
2. The laptop daemon asks the phone to open a window (`adb_open`). phonectl
   holds `WRITE_SECURE_SETTINGS` (granted once with `pm grant`), so it can set
   `Settings.Global.adb_wifi_enabled = 1` itself. Android only keeps wireless
   debugging on for Wi-Fi networks the owner has trusted ("Always allow on
   this network"); elsewhere the window does not open and the laptop retries
   on a later connection.
3. adbd's TLS port is random and apps cannot read it (no property, no
   `/proc/net`), so the app finds it by connecting to loopback ports and
   looking for the one that answers an ADB `CNXN` with `STLS`. This takes
   well under a second. It replies with the port number only.
4. The daemon connects to the phone's Tailscale address on that port with its
   own ADB key (paired once) and runs one fixed command:
   `am instrument com.daanh.phonectl/.ShellWatcher`.
5. The restarted process brings the watcher up (steps 1 to 6 above) and
   switches wireless debugging off. If anything goes wrong, a deadline stored
   in the app's preferences switches it off after two minutes, also across the
   process restart.

Wireless debugging is on for a few seconds per restart, never in daily use.
USB debugging (`adb_enabled`) is never touched.

## Cost

The watcher is a binder listener inside a process that is running anyway for
the link. It wakes only when something is copied. The port scan runs only
during a restart.

## What it does not do

- It does not use root, Shizuku, an accessibility service, logcat or a
  custom keyboard; Gboard (or any keyboard) stays the keyboard.
- Android still shows its "phonectl pasted from your clipboard" toast the
  first time phonectl reads a clip another app wrote. The permission that
  suppresses it (`SUPPRESS_CLIPBOARD_ACCESS_NOTIFICATION`) is not held by the
  shell, so it cannot be borrowed.
- Clips marked sensitive (`ClipDescription.EXTRA_IS_SENSITIVE`, which
  password managers set) are never sent.

## Trying it by hand

```sh
adb shell am instrument com.daanh.phonectl/.ShellWatcher
adb shell dumpsys activity processes | grep -A1 ShellWatcher
adb shell settings put global adb_wifi_enabled 0   # watcher keeps running
```

# Silent updates of the owner's own apps

The same shell identity also holds `INSTALL_PACKAGES`, which lets an installer
skip Android's "Do you want to update this app?" prompt. phonectl borrows it
only for the seconds of one install:

1. Once an hour when the link connects, and on `phonectl update-apps`, the app
   reads the phone store's listing for each package in `OwnApps.PINS`
   (`http://<pi>:8766/apps/<package>/`, over Tailscale) and picks the highest
   version code from the file names (`<package>-<version>-<code>.apk`).
2. If that is newer than the installed version and the watcher is running, it
   downloads the APK into its private cache and checks it with
   `getPackageArchiveInfo`: listed package, already installed, higher version
   code, every signer equal to the pinned certificate SHA-256.
3. Only then `ShellClip.borrowInstallPermission()` adopts
   `READ_CLIPBOARD_IN_BACKGROUND` + `INSTALL_PACKAGES`, and a `PackageInstaller`
   session with `USER_ACTION_NOT_REQUIRED` is committed.
4. When Android reports the result (`UpdateReceiver`, not exported), the
   identity goes back to the clipboard permission alone; a timer does the same
   after two minutes if no result came. The result is listed under "Recent
   laptop actions" and sent to the laptop as a `store_update` event.

phonectl is updated last, because installing it ends phonectl's own process
(and with it the watcher, which the laptop then restarts as after any update).
Without the watcher nothing is installed and Obtainium's normal prompt stays
the way to update. The switch "Install updates of my own apps without asking"
in the app turns the feature off.

# The command gate

Every call from the laptop goes through `Actions.admit` in `Link` before any
feature sees it: the method must be in `Actions.RULES`, under its per-minute
limit, and harmful methods need the phone unlocked. The app shows the last
calls (and refusals) under "Recent laptop actions".
