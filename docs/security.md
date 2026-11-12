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
- Borrowed shell permissions: `ShellWatcher.kt`, the only file that touches
  `UiAutomation`. `READ_CLIPBOARD_IN_BACKGROUND` while the watcher runs;
  `INSTALL_PACKAGES` added only for the seconds of a silent store update.
- Laptop commands the phone accepts: `Actions.RULES` in `Actions.kt`
  (whitelist, rate limit, lock requirement per method).
- Apps phonectl may install and their pinned signing keys: `OwnApps.PINS` in
  `StoreUpdater.kt`.
- Secure setting writes: `AdbWindow.kt` (only `adb_wifi_enabled`).
- Phone methods the laptop CLI may call: `PHONE_CALLS` in
  `crates/phonectl/src/daemon/ipc.rs`.
- The one command the daemon runs over ADB: `START_COMMAND` in
  `crates/phonectl/src/daemon/watcher.rs`.

Tests that fail when one of these widens:
`ExportedComponentsTest` (Android unit test: exported components, their
guards, no providers, one instrumentation, only the listed adopted
permissions and never the adopt-everything form, no other UiAutomation use,
UiAutomation private to `ShellWatcher.kt`), `ActionsTest` (unknown methods
refused, every method a feature answers has a rule and a rate limit, the gate
runs before any feature), `StoreUpdaterTest` (full pins, verify before
install, permission returned on every path, only the phone switches silent
updates), `ipc::tests` and `watcher::tests` (Rust).

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

### Command gate (every laptop call)

- **Door:** the paired laptop (and anything that can use the laptop daemon's
  socket) sends method calls to the phone.
- **Kept shut:**
  - `Link` hands every call to `Actions.admit` before any feature sees it.
    Methods not in `Actions.RULES` are refused ("unknown method"), so a new
    feature cannot be reached until it is given a rule.
  - Each method has its own per-minute limit (sliding window); calls over it
    are refused.
  - Methods marked harmful (`type_text` today; install, screen and hotspot
    as they are added) are refused while the phone is locked.
  - Every call, run or refused, is written to "Recent laptop actions" in the
    app (last 100, kept across restarts, repeats folded into one line with a
    count), and to the app log. Arguments are not shown (they can hold
    typed text); the method and its outcome are.
- **Residual risk:** someone with the link key can still run the allowed
  methods within their limits while the phone is unlocked. The list shows
  it afterwards.
- **Turn off:** unpair in the app (or force-stop it); the laptop then has no
  way in.

### Silent updates of the owner's own apps (shell INSTALL_PACKAGES, borrowed per install)

- **Door:** while a store update installs, phonectl's uid passes the
  `INSTALL_PACKAGES` check, so Android installs without a prompt.
- **Kept shut:**
  - Only apps listed in `OwnApps.PINS` (vector_phone, crew, krentenweger,
    phonectl Keyboard, phonectl), only if already installed (no new apps),
    only a higher version code than the installed one (no downgrades), and
    every signer must match that package's pinned certificate SHA-256. The
    APK is checked before the permission is borrowed. Android's own
    same-signature rule applies on top.
  - The store is the owner's phone store on the Pi, reached over Tailscale
    (WireGuard). Plain HTTP is allowed to that one address only
    (`network_security_config.xml`); redirects are not followed, the page is
    capped at 512 KB and an APK at 300 MB.
  - `INSTALL_PACKAGES` is adopted next to the clipboard permission in
    `ShellClip.borrowInstallPermission` and dropped again when Android
    reports the install result, when the install fails to start, and after
    two minutes at the latest. One install per check, one check per 10
    minutes (plus once an hour on connect).
  - The laptop can only ask for a check (`store_check`, `phonectl
    update-apps`); it cannot name an app, a URL or a file. Switching silent
    updates on or off is only possible in the app on the phone.
  - Without the watcher nothing is installed: Obtainium still offers the
    update with Android's prompt. If Android still wants a tap (another
    installer owns updates for that app), the session is abandoned, not
    shown.
  - Every install, refusal and result is listed under "Recent laptop
    actions".
- **Residual risk:** phonectl, the keyboard, crew and vector_phone are signed
  with the laptop's Android debug key (`~/.android/debug.keystore`), which AI
  agents in Room B can read, and agents can reach the Pi. An agent that signs
  a newer build and puts it in the store directly (not through `airlock
  action phone-publish`) gets it installed without a tap. Android's update
  rule already lets that key update these apps with one tap, so the pin adds
  no new signer; it removes the tap. Closing this fully needs a release key
  that only the owner's side holds (for example used by `phone-publish`), and
  a move of the installed apps to it.
- **Turn off:** phonectl app > Updates > switch off "Install updates of my
  own apps without asking". Force-stopping phonectl also ends it (no
  watcher, no borrowed permission).

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
  `type_text`, `store_check`). It cannot open wireless debugging or run ADB
  commands. The phone applies its own gate on top (above).
- `update` sends an APK to the phone; the app refuses it unless it is
  phonectl, signed with the pinned key and newer, and Android then asks the
  owner to confirm the install.
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
| Laptop sends a method no feature lists in `Actions.RULES` | Refused before any feature sees it, listed as refused (test `unknownMethodsAreRefused`, `linkAdmitsBeforeDispatch`) |
| Laptop floods `ring` | Refused after 6 per minute, listed (test `rateLimitIsPerMethodAndSlides`) |
| Laptop sends `type_text` while the phone is locked | Refused: "phone is locked" |
| Store serves an older phonectl, an unlisted package or one signed with another key | Refused before the install permission is borrowed (`OwnApps.verify`), listed |
| Store serves a page with a redirect to another host | Not followed |
| Laptop pushes an APK of another app with `update` | Refused: only phonectl, pinned key, newer |
| Laptop tries to switch silent updates on | No method for it; only the app's switch writes it (test `onlyThePhoneSwitchesSilentUpdates`) |
