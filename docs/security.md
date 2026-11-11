# Security

phonectl links one phone to one laptop. The phone side holds a few strong
capabilities, so this page lists every one of them: what door it opens, how
the door is kept shut, what risk is left, and how to turn it off.

## Threat model

Who we defend against:

- **Other apps on the phone.** They must not be able to read the clipboard
  through phonectl, trigger any phonectl action (typing, installs, hotspot,
  screen, ring, opening links), or change phonectl's pairing.
- **Web pages** opened on the phone (they can fire intents through links).
- **Other devices on the tailnet or the local network.** They must not be
  able to talk to the phone app or the laptop daemon without the link key.
- **Other local users on the laptop.**
- **AI agents on the laptop** (they run in a separate sandbox, "Room B").
  They must not be able to drive privileged phone actions without the owner.

Out of scope: someone who has the unlocked phone in hand, root on either
device, or the owner's laptop account.

## Where the capabilities are listed in code

- Phone permissions: `android/app/src/main/AndroidManifest.xml`.
- Borrowed shell permission: `ShellWatcher.kt` (one call,
  `adoptShellPermissionIdentity(READ_CLIPBOARD_IN_BACKGROUND)`).
- Secure setting writes: `AdbWindow.kt` (only `adb_wifi_enabled`).
- Phone methods the laptop CLI may call: `PHONE_CALLS` in
  `crates/phonectl/src/daemon/ipc.rs`.
- The one command the daemon runs over ADB: `START_COMMAND` in
  `crates/phonectl/src/daemon/watcher.rs`.

Tests that fail when one of these widens:
`ExportedComponentsTest` (Android unit test: exported components, their
guards, no providers, one instrumentation, one adopted permission, no other
UiAutomation use), `ipc::tests` and `watcher::tests` (Rust).

## Capabilities

### Background clipboard reads (shell identity)

- **Door:** while `ShellWatcher` runs, phonectl's uid passes the
  `READ_CLIPBOARD_IN_BACKGROUND` check, so phonectl reads every copy.
- **Kept shut:**
  - Only that one permission is adopted, never the no-argument form that
    adopts all shell permissions.
  - The `UiAutomation` object is used for that single call; no input
    injection, screenshots, window queries or shell commands. A unit test
    checks the source for other uses.
  - On release builds of Android, ActivityManager only lets root, shell or
    system start an instrumentation ("not allowed because it's not started
    from SHELL", `ActivityManagerService.startInstrumentation`). Another app
    calling `Context.startInstrumentation` is refused, and the shell-owned
    `UiAutomationConnection` that makes adoption possible only comes from
    `am instrument`.
  - Clips go only into `ClipSync`, which sends them only to the paired
    laptop over the authenticated link. No exported component, intent,
    provider or binder exposes them.
  - Sensitive clips (`EXTRA_IS_SENSITIVE`, set by password managers) are
    never sent. Clip contents are never logged (lengths only).
- **Residual risk:** code running inside phonectl's process holds the shell
  `UiAutomation` connection while the watcher runs. A bug that lets someone
  run code inside phonectl would let them use it. phonectl loads no plug-ins,
  no WebView and no code from the network.
- **Turn off:** force-stop phonectl or reboot (the watcher stops), and revoke
  the grant below so it is not restarted.

### WRITE_SECURE_SETTINGS (wireless debugging windows)

- **Door:** this permission can change any secure or global setting.
- **Kept shut:** the app only ever writes `adb_wifi_enabled` (`AdbWindow`).
  It switches it on only when the authenticated laptop asks (`adb_open`, not
  callable from the laptop CLI), and off as soon as the watcher runs, after
  two minutes at the latest (a deadline stored on disk, so it also holds if
  the process restarts). A window someone else opened is left alone. Android
  itself refuses wireless debugging on Wi-Fi networks the owner has not
  trusted. USB debugging (`adb_enabled`) and Developer options are never
  touched.
- **Residual risk:** for those seconds, adbd is reachable on the phone's
  Wi-Fi and Tailscale addresses; it only accepts paired keys.
- **Turn off:** `adb shell pm revoke com.daanh.phonectl android.permission.WRITE_SECURE_SETTINGS`.

### The laptop's ADB key

- **Door:** a paired key gets a shell on the phone whenever wireless
  debugging is on.
- **Kept shut:** stored in `~/.local/share/phonectl/adbkey`, mode 0600 in a
  0700 directory (refused if others can read it). The daemon runs exactly one
  fixed command with it (`am instrument …ShellWatcher`); the phone only
  supplies a port number, which is checked to be a number from 1024 to 65535.
  Nothing the phone sends ends up in a command line.
- **Turn off:** Wireless debugging > Paired devices > forget this laptop.

### The link (Tailscale or Bluetooth)

- Mutual HMAC-SHA256 challenge-response with a 32-byte key shared at pairing;
  both sides send fresh random nonces and the proofs are bound to the role
  ("laptop" / "phone"), so a recorded handshake cannot be replayed or
  reflected. Proofs are compared in constant time on both sides. There is
  one protocol version and no fallback to weaker modes.
- Messages after the handshake rely on the transport for confidentiality
  and integrity (WireGuard for Tailscale, link encryption for Bluetooth).
- Keys: laptop `~/.config/phonectl/key` 0600; phone in app-private storage
  with backups disabled (`allowBackup=false`).
- Pairing from outside the app (a `phonectl:` link, an intent extra, a
  pairing string on the clipboard) only opens a dialog showing the laptop's
  name and address; nothing changes until the owner taps Pair.
- **Residual risk:** the phone-side key is not wrapped by the Android
  Keystore; anyone with root on the phone can read it.

### Laptop daemon

- The socket lives in `$XDG_RUNTIME_DIR/phonectl` (mode 0700, owner only).
- Through it, the CLI can call only the phone methods in `PHONE_CALLS`
  (`ring`, `ringer`, `media_action`, `test_notification`, `diag`,
  `type_text`). It cannot open wireless debugging or run ADB commands.
- `update` sends an APK to the phone; Android only installs it over phonectl
  if it is signed with the same key, and refuses downgrades.
- The AI sandbox (Room B) does not see the socket unless the owner shares
  `$XDG_RUNTIME_DIR/phonectl` with it.

### Other permissions

- `READ_LOGS` (old log-based clipboard fallback, granted once over ADB):
  only used when the watcher is not available.
- `QUERY_ALL_PACKAGES`: to open the right app from a notification action.
- Notification listener: binds only via `BIND_NOTIFICATION_LISTENER_SERVICE`
  (system only).

## Exported components

| Component | Why exported | Guard |
|---|---|---|
| `MainActivity` | Launcher icon; `phonectl:` pairing links | Pairing always asks the owner first |
| `ShareActivity` | "Send to laptop" in the share sheet | Sends the shared text to the paired laptop only |
| `NotifListener` | Notification access | `BIND_NOTIFICATION_LISTENER_SERVICE` (system only) |
| `ClipTile` | Quick settings tile | `BIND_QUICK_SETTINGS_TILE` (system only) |
| `KeyboardBridge` | phonectl Keyboard → app | Signature permission `BRIDGE` |

No content providers. The `ShellWatcher` instrumentation is declared but can
only be started from a shell (see above).

Known residual: any app can launch `ShareActivity` directly and so put text
on the laptop's clipboard. It cannot read anything back.

## Attacker review

Each line: what was tried, and what happened.

| Attempt | Result |
|---|---|
| Another app starts `ShellWatcher` with `startInstrumentation` | Refused by ActivityManager on user builds: only root, shell or system may start an instrumentation |
| Web page or app sends `phonectl:` link to re-pair phonectl to an attacker laptop | Before 0.3.0: paired silently. Now: confirm dialog with the laptop's name and address |
| Another app puts a pairing string on the clipboard while phonectl is unpaired | Before 0.3.0: paired silently on focus. Now: confirm dialog |
| Laptop CLI (or an agent with socket access) calls `adb_open` | Refused: not in `PHONE_CALLS` (test `cli_cannot_reach_debugging_or_install_calls`) |
| Phone (or someone who stole the link key) sends a port like `"1; reboot"` | Rejected: must be a JSON number 1024 to 65535 (test `port_must_be_a_plain_unprivileged_number`) |
| Laptop goes away after `adb_open` | Wireless debugging off after two minutes (deadline survives process restarts) |
| Watcher keeps dying after a start | Laptop waits 1, 5, 15, 60 then 180 minutes between starts; a start that went through is not retried on reconnect |
